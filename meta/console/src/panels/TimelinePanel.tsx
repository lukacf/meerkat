import React from "react";
import { describeFailure } from "../lib/failure-summary";
import type { ConsoleFrame } from "../types";

interface TimelinePanelProps {
  frames: ConsoleFrame[];
}

type TlType = "dispatch" | "gate" | "warn" | "topology" | "lifecycle" | "interaction";

const INTERNAL_TIMELINE_EVENTS = new Set([
  "keep-alive",
  "snapshot_complete",
  "snapshot_started",
  "subscribed",
]);

interface TlEntry {
  time: string;
  type: TlType;
  text: string;
  who: string;
}

function classifyFrame(frame: ConsoleFrame): TlType {
  const ev = frame.event;
  if (ev === "gating_decision" || ev.startsWith("gate_")) return "gate";
  if (ev === "run_failed" || ev === "interaction_failed") return "warn";
  if (ev === "route_changed" || ev === "topology_updated") return "topology";
  if (ev === "member_ready" || ev === "member_retired" || ev === "state_changed") return "lifecycle";
  if (ev === "interaction_complete" || ev === "interaction_started") return "interaction";
  return "dispatch";
}

function formatType(type: TlType): string {
  switch (type) {
    case "gate": return "Gate";
    case "warn": return "Warning";
    case "topology": return "Topology";
    case "lifecycle": return "Lifecycle";
    case "interaction": return "Interaction";
    default: return "Dispatch";
  }
}

function formatTime(tsMs?: number): string {
  if (!tsMs) return "—";
  const d = new Date(tsMs);
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${hh}:${mm}`;
}

function summarizeFrame(frame: ConsoleFrame): string {
  const ev = frame.event;
  const data = (frame.data || {}) as Record<string, unknown>;
  const shortInteraction = String(frame.interactionId || "").slice(0, 8);
  switch (ev) {
    case "interaction_complete":    return shortInteraction ? `Completed ${shortInteraction}` : "Completed";
    case "interaction_failed":      return `Failed: ${describeFailure(data)}`;
    case "interaction_started":     return shortInteraction ? `Started ${shortInteraction}` : "Started";
    case "gating_decision":         return `Gate ${String(data.decision || "")}: ${String(data.action_id || data.pending_id || "")}`;
    case "member_ready":            return `Member ready`;
    case "member_retired":          return `Member retired`;
    case "state_changed":           return `State → ${String(data.state || data.new_state || "")}`;
    case "route_changed":           return `Route updated`;
    default:                        return ev.replace(/_/g, " ");
  }
}

export function TimelinePanel({ frames }: TimelinePanelProps): React.JSX.Element {
  const entries: TlEntry[] = React.useMemo(() => {
    const todayMs = (() => {
      const d = new Date();
      d.setHours(0, 0, 0, 0);
      return d.getTime();
    })();
    return frames
      .filter((f) => !INTERNAL_TIMELINE_EVENTS.has(f.event))
      .filter((f) => (f.timestampMs || 0) >= todayMs)
      .slice(0, 80)
      .map((f) => ({
        time: formatTime(f.timestampMs),
        type: classifyFrame(f),
        text: summarizeFrame(f),
        who: f.identity || "_system",
      }));
  }, [frames]);

  const today = new Date();
  const dateLabel = today.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });

  return (
    <div className="tl" data-testid="timeline-panel">
      <div className="tl__head">
        <h2>Today</h2>
        <p>· {entries.length} events · {dateLabel}</p>
      </div>
      <div className="tl__body">
        {entries.length === 0 && (
          <div style={{ gridColumn: "1 / -1", padding: "40px 0", color: "var(--ink-dim)", fontFamily: "var(--mono)", fontSize: 12, textAlign: "center" }}>
            No events yet today.
          </div>
        )}
        {entries.map((e, i) => (
          <div className="tl__row" data-type={e.type} key={i}>
            <div className="tl__time">{e.time}</div>
            <div className="tl__rail"><span className="tl__dot" /></div>
            <div className="tl__card">
              <div>
                <span className="tl__type">{formatType(e.type)}</span>{" "}
                <span>{e.text}</span>
              </div>
              <div className="tl__who">{e.who}</div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
