import React from "react";
import type { ConsoleFrame } from "../types";
// Pure kit helpers only: this panel's node test bundle cannot resolve React
// from the kit package, so it uses the kit's CSS classes in its own markup.
import {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  type AgentDirectory,
} from "../../../packages/console-components/src/panel-kit/agents";
import { formatAbsoluteTime, joinMeta, plural } from "../../../packages/console-components/src/panel-kit/format";
import { copyTextToClipboard } from "../../../packages/console-components/src/shared";

interface LogsPanelProps {
  frames: ConsoleFrame[];
  /** Resolves identity keys to display names; keys show without their kind
   * prefix when absent. */
  agentDirectory?: AgentDirectory;
}

type Level = "info" | "warn" | "error";

const INTERNAL_LOG_EVENTS = new Set([
  "keep-alive",
  "snapshot_complete",
  "snapshot_started",
  "subscribed",
]);

const LEVEL_FILTERS: ReadonlyArray<{ id: "all" | Level; label: string }> = [
  { id: "all", label: "All" },
  { id: "info", label: "Info" },
  { id: "warn", label: "Warnings" },
  { id: "error", label: "Errors" },
];

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
    if (out.length > 10_000) return out.slice(0, 10_000) + "\n... (truncated)";
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

/** An ID in the expanded detail, in the kit's technical ID markup: muted
 * mono, middle-truncated, the full value in the tooltip and for screen
 * readers, and a copy button. */
function LogTechnicalId({ value, label }: { value: string; label: string }): React.JSX.Element {
  const [copied, setCopied] = React.useState(false);
  const tail = value.length > 12 ? value.slice(-6) : "";
  const head = tail ? value.slice(0, -6) : value;
  React.useEffect(() => {
    if (!copied) return undefined;
    const timer = window.setTimeout(() => setCopied(false), 1600);
    return () => window.clearTimeout(timer);
  }, [copied]);
  return (
    <span className="cc-tech-id" title={value}>
      <span className="cc-tech-id__text" aria-hidden="true">
        <span className="cc-tech-id__head">{head}</span>
        {tail ? <span className="cc-tech-id__tail">{tail}</span> : null}
      </span>
      <span className="cc-visually-hidden">{value}</span>
      <button
        type="button"
        className="cc-copy-btn cc-tech-id__copy"
        aria-label={copied ? "Copied" : `Copy ${label}`}
        title={copied ? "Copied" : `Copy ${label}`}
        data-copied={copied ? "true" : undefined}
        onClick={() => {
          void copyTextToClipboard(value).then((done) => { if (done) setCopied(true); });
        }}
      >
        <svg aria-hidden="true" focusable="false" viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
          {copied ? (
            <path d="M3.5 8.5l3 3 6-7" />
          ) : (
            <>
              <rect height="9" rx="1.5" width="8" x="5.5" y="5" />
              <path d="M10.5 3.5V3a1.5 1.5 0 0 0-1.5-1.5H4A1.5 1.5 0 0 0 2.5 3v6A1.5 1.5 0 0 0 4 10.5h.5" />
            </>
          )}
        </svg>
      </button>
    </span>
  );
}

export function LogsPanel({ frames, agentDirectory = EMPTY_AGENT_DIRECTORY }: LogsPanelProps): React.JSX.Element {
  const [q, setQ] = React.useState("");
  const [lvl, setLvl] = React.useState<"all" | Level>("all");
  const [expanded, setExpanded] = React.useState<Set<string>>(new Set());
  // Paused keeps the frames shown when the operator paused, so rows stop
  // moving while they read; following live shows every new frame.
  const [pausedFrames, setPausedFrames] = React.useState<ConsoleFrame[] | null>(null);
  const searchId = React.useId();
  const shownFrames = pausedFrames ?? frames;

  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const rows = React.useMemo(() => {
    return shownFrames
      .filter(isLogFrameVisible)
      .map((f) => ({ f, level: levelFor(f), who: logIdentityLabel(f.identity, agentDirectory) }))
      .filter(({ f, level, who }) => {
        if (lvl !== "all" && level !== lvl) return false;
        if (!q) return true;
        const needle = q.toLowerCase();
        return (
          f.event.toLowerCase().includes(needle) ||
          (f.identity || "").toLowerCase().includes(needle) ||
          who.toLowerCase().includes(needle)
        );
      });
  }, [shownFrames, q, lvl, agentDirectory]);

  const counts = React.useMemo(() => {
    const c = { info: 0, warn: 0, error: 0 };
    shownFrames.filter(isLogFrameVisible).forEach((f) => { c[levelFor(f)]++; });
    return c;
  }, [shownFrames]);
  const visibleTotal = counts.info + counts.warn + counts.error;
  // New frames arrive at the front; while paused, count those ahead of the
  // newest frame on screen.
  const newWhilePaused = pausedFrames
    ? (() => {
        const newest = pausedFrames[0];
        const index = newest ? frames.indexOf(newest) : frames.length;
        return index >= 0 ? index : frames.length;
      })()
    : 0;
  const filtered = Boolean(q) || lvl !== "all";

  return (
    <section className="cc-panel logs" data-testid="logs-panel">
      <header className="cc-panel-head">
        <div className="cc-panel-head__text">
          <h2 className="cc-panel-head__title">Logs</h2>
          <div className="cc-panel-head__sub">
            {joinMeta(
              filtered ? `${rows.length} of ${plural(visibleTotal, "event")}` : plural(visibleTotal, "event"),
              pausedFrames ? (newWhilePaused ? `Paused, ${plural(newWhilePaused, "new event")}` : "Paused") : "Live",
            )}
          </div>
        </div>
        <div className="cc-panel-head__actions">
          <button
            type="button"
            className="cc-btn logs__follow"
            aria-pressed={pausedFrames === null}
            onClick={() => setPausedFrames((current) => (current ? null : frames))}
          >
            <span className={pausedFrames ? "cc-dot" : "cc-dot is-live"} data-tone={pausedFrames ? "neutral" : "ok"} aria-hidden="true" />
            Follow live
          </button>
        </div>
      </header>
      <div className="cc-toolbar logs__toolbar">
        <div className="cc-field cc-search">
          <label htmlFor={searchId} className="cc-visually-hidden">Filter logs</label>
          <span className="cc-search__box">
            <svg className="cc-glyph cc-search__icon" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
              <circle cx="11" cy="11" r="6" />
              <path d="m20 20-4.35-4.35" />
            </svg>
            <input
              id={searchId}
              type="search"
              className="cc-input cc-search__input"
              placeholder="Event or agent"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          </span>
        </div>
        <div className="cc-seg" role="group" aria-label="Level">
          {LEVEL_FILTERS.map((filter) => {
            const count = filter.id === "all" ? visibleTotal : counts[filter.id];
            return (
              <button
                key={filter.id}
                type="button"
                className={lvl === filter.id ? "cc-seg__btn is-active" : "cc-seg__btn"}
                aria-pressed={lvl === filter.id}
                onClick={() => setLvl(filter.id)}
              >
                {filter.label} <span className="cc-count">{count}</span>
              </button>
            );
          })}
        </div>
      </div>
      <div className="logs__body">
        <div className="logs__stream">
          {rows.map(({ f, level, who }, i) => {
            const key = f.id || `${f.event}:${f.timestampMs}:${i}`;
            const isOpen = expanded.has(key);
            const hasStructured = hasStructuredOutput(f);
            const identity = f.identity?.trim() || "";
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
                  <span className="logline__chevron" aria-hidden="true" />
                  <span className="logline__t" title={formatAbsoluteTime(f.timestampMs) || undefined}>{formatTime(f.timestampMs)}</span>
                  <span className={`logline__lvl logline__lvl--${level}`}>{level.toUpperCase()}</span>
                  <span className="cc-agent logline__src" title={identity || undefined}>
                    <span className="cc-agent__name">{who}</span>
                  </span>
                  <span className="logline__evt" title={f.event}>{f.event}</span>
                  <span className="logline__ctx" title={f.interactionId ? `Interaction ${f.interactionId}` : undefined}>
                    {f.interactionId || ""}
                  </span>
                  <span className="logline__msg">{summarizeLogFrame(f)}</span>
                  {hasStructured && (
                    <span className="logline__badge" title="Carries structured output">structured</span>
                  )}
                </button>
                {isOpen && (
                  <div className="logline__detail" data-testid={`log-line:${f.id || i}:detail`}>
                    <dl className="cc-kv logline__facts">
                      <div className="cc-kv__row">
                        <dt>Agent</dt>
                        <dd>
                          {who}
                          {identity && identity !== who ? <span className="logline__key">{identity}</span> : null}
                        </dd>
                      </div>
                      <div className="cc-kv__row">
                        <dt>Event</dt>
                        <dd className="logline__code">{f.event}</dd>
                      </div>
                      {f.interactionId ? (
                        <div className="cc-kv__row">
                          <dt>Interaction</dt>
                          <dd><LogTechnicalId value={f.interactionId} label="interaction ID" /></dd>
                        </div>
                      ) : null}
                      {f.timestampMs ? (
                        <div className="cc-kv__row">
                          <dt>Time</dt>
                          <dd>{formatAbsoluteTime(f.timestampMs)}</dd>
                        </div>
                      ) : null}
                    </dl>
                    <pre className="logline__data">{formatFrameData(f)}</pre>
                  </div>
                )}
              </div>
            );
          })}
          {rows.length === 0 && (
            <div className="cc-panel-empty" data-kind={filtered ? "filtered" : "empty"}>
              <span className="cc-panel-empty__mark" aria-hidden="true">
                <svg className="cc-glyph" aria-hidden="true" focusable="false" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  {filtered ? (
                    <>
                      <circle cx="11" cy="11" r="6" />
                      <path d="m20 20-4.35-4.35" />
                    </>
                  ) : (
                    <>
                      <circle cx="12" cy="12" r="9" />
                      <path d="M12 10v6" />
                      <path d="M12 7h.01" />
                    </>
                  )}
                </svg>
              </span>
              <p className="cc-panel-empty__title">{filtered ? "No matching events" : "No events yet"}</p>
              <p className="cc-panel-empty__body">
                {filtered
                  ? "Nothing in the log matches this filter."
                  : "Events from agents and the console appear here as they happen."}
              </p>
              {filtered ? (
                <div className="cc-panel-empty__action">
                  <button type="button" className="cc-btn cc-btn--sm" onClick={() => { setQ(""); setLvl("all"); }}>
                    Clear filters
                  </button>
                </div>
              ) : null}
            </div>
          )}
        </div>
      </div>
    </section>
  );
}
