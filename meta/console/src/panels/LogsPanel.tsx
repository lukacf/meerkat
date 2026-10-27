import React from "react";
import type { ConsoleFrame } from "../types";

interface LogsPanelProps {
  frames: ConsoleFrame[];
}

type Level = "info" | "warn" | "error";

const INTERNAL_LOG_EVENTS = new Set([
  "keep-alive",
  "snapshot_complete",
  "snapshot_started",
  "subscribed",
]);

export function isLogFrameVisible(frame: ConsoleFrame): boolean {
  if (INTERNAL_LOG_EVENTS.has(frame.event)) return false;
  return true;
}

function levelFor(frame: ConsoleFrame): Level {
  const ev = frame.event;
  if (ev.includes("failed") || ev.includes("error") || ev.includes("crash")) return "error";
  if (ev.includes("warn") || ev.includes("degraded") || ev.includes("gating_decision")) return "warn";
  return "info";
}

function formatTime(tsMs?: number): string {
  if (!tsMs) return "—";
  const d = new Date(tsMs);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  const ss = String(d.getSeconds()).padStart(2, "0");
  const ms = String(d.getMilliseconds()).padStart(3, "0");
  return `${hh}:${mm}:${ss}.${ms}`;
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

/// Pretty-print a frame's `data` field for the expanded row. JSON
/// stringify with 2-space indent, but cap at ~10 KB so a giant tool
/// result doesn't blow up the panel.
function formatFrameData(frame: ConsoleFrame): string {
  const data = sanitizeLogFrameData(frame.data ?? null);
  if (data === null || data === undefined) return "(no data)";
  try {
    const out = JSON.stringify(data, null, 2);
    if (out.length > 10_000) return out.slice(0, 10_000) + "\n… (truncated)";
    return out;
  } catch {
    return String(data);
  }
}

/// Heuristic: does this frame carry a `structured_output` field worth
/// flagging in the row summary so operators notice it without having
/// to expand every row? Surfaces the new schema-extraction result on
/// `interaction_complete` / `run_completed` / `flow_completed` events.
function hasStructuredOutput(frame: ConsoleFrame): boolean {
  const d = frame.data;
  if (!d || typeof d !== "object") return false;
  return (d as Record<string, unknown>).structured_output != null;
}

export function LogsPanel({ frames }: LogsPanelProps): React.JSX.Element {
  const [q, setQ] = React.useState("");
  const [lvl, setLvl] = React.useState<"all" | Level>("all");
  const [expanded, setExpanded] = React.useState<Set<string>>(new Set());

  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const rows = React.useMemo(() => {
    return frames
      .filter(isLogFrameVisible)
      .map((f) => ({ f, level: levelFor(f) }))
      .filter(({ f, level }) => {
        if (lvl !== "all" && level !== lvl) return false;
        if (!q) return true;
        const needle = q.toLowerCase();
        return (
          f.event.toLowerCase().includes(needle) ||
          (f.identity || "").toLowerCase().includes(needle)
        );
      });
  }, [frames, q, lvl]);

  const counts = React.useMemo(() => {
    const c = { info: 0, warn: 0, error: 0 };
    frames.filter(isLogFrameVisible).forEach((f) => { c[levelFor(f)]++; });
    return c;
  }, [frames]);
  const visibleTotal = counts.info + counts.warn + counts.error;

  return (
    <div className="view logs" data-testid="logs-panel">
      <div className="view__head">
        <h2>Logs</h2>
        <span className="view__sub">{rows.length} of {visibleTotal} events · live</span>
        <span className="view__spacer" />
        <input
          className="view__search"
          placeholder="Filter event, identity…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <div className="view__segs">
          <button className={lvl === "all" ? "is-active" : ""} onClick={() => setLvl("all")}>
            all <span className="n">{visibleTotal}</span>
          </button>
          <button className={lvl === "info" ? "is-active" : ""} onClick={() => setLvl("info")}>
            info <span className="n">{counts.info}</span>
          </button>
          <button className={`warn ${lvl === "warn" ? "is-active" : ""}`} onClick={() => setLvl("warn")}>
            warn <span className="n">{counts.warn}</span>
          </button>
          <button className={`bad ${lvl === "error" ? "is-active" : ""}`} onClick={() => setLvl("error")}>
            err <span className="n">{counts.error}</span>
          </button>
        </div>
      </div>
      <div className="logs__body">
        <div className="logs__stream">
          {rows.map(({ f, level }, i) => {
            const key = f.id || `${f.event}:${f.timestampMs}:${i}`;
            const isOpen = expanded.has(key);
            const hasStructured = hasStructuredOutput(f);
            return (
              <div
                key={key}
                className={`logline logline--${level}${isOpen ? " is-open" : ""}`}
                data-testid={`log-line:${f.id || i}`}
              >
                <button
                  type="button"
                  className="logline__row"
                  onClick={() => toggle(key)}
                  aria-expanded={isOpen}
                  data-testid={`log-line:${f.id || i}:toggle`}
                >
                  <span className="logline__chevron">{isOpen ? "▾" : "▸"}</span>
                  <span className="logline__t">{formatTime(f.timestampMs)}</span>
                  <span className={`logline__lvl logline__lvl--${level}`}>{level.toUpperCase()}</span>
                  <span className="logline__src">{f.identity || "_system"}</span>
                  <span className="logline__evt">{f.event}</span>
                  <span className="logline__ctx dim">{f.interactionId ? `int=${f.interactionId.slice(0, 8)}` : ""}</span>
                  <span className="logline__msg">{summarizeLogFrame(f)}</span>
                  {hasStructured && (
                    <span className="logline__badge" title="Carries structured_output">↳ struct</span>
                  )}
                </button>
                {isOpen && (
                  <pre className="logline__detail" data-testid={`log-line:${f.id || i}:detail`}>
                    {formatFrameData(f)}
                  </pre>
                )}
              </div>
            );
          })}
          {rows.length === 0 && (
            <div style={{ padding: 24, color: "var(--ink-dim)", fontFamily: "var(--mono)", fontSize: 12 }}>
              No matching events.
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
