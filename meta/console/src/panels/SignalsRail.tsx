import React from "react";
import { describeMemoryTimelineEvent, stripPeerTransportScaffold } from "../lib/adapters";
import { describeFailure } from "../lib/failure-summary";
import { memberKickoffNotice, memberKickoffPhaseLabel, type MemberKickoffNotice } from "@console-core";
import type { ConsoleFrame, ConsoleRailFilterPresetConfig } from "../types";
import { countRender } from "../lib/render-counts";

interface SignalsRailProps {
  frames: ConsoleFrame[];
  collapsed: boolean;
  filterPresets?: ConsoleRailFilterPresetConfig[];
  activePresetId?: string;
  emptyText?: string;
  watchedIdentities?: Set<string>;
  onPresetChange?: (presetId: string) => void;
  onSelect?: (frame: ConsoleFrame) => void;
}

type Severity = "critical" | "warning" | "info";

interface Signal {
  id: string;
  severity: Severity;
  label: string;
  detail: string;
  agent: string;
  at: string;
  raw: ConsoleFrame;
}

interface SignalGroup {
  id: string;
  severity: Severity;
  title: string;
  detail: string;
  agent: string;
  at: string;
  items: Signal[];
}

const DEFAULT_FILTER_PRESETS: ConsoleRailFilterPresetConfig[] = [
  { id: "all", label: "All" },
  { id: "warning", label: "Attn", alertLevels: ["warning", "critical"] },
  { id: "critical", label: "Crit", alertLevels: ["critical"] },
];

const PEER_TOOLS = new Set(["send_request", "send_message", "send_response"]);
const LOW_VALUE_REPLIES = new Set(["done", "ok", "okay", "acknowledged"]);
const LOW_VALUE_REPLY_PATTERNS = [
  /^acknowledged[.!]?\s+(i[’']?m\s+)?(online|acting as|ready|scribe|incident commander)/i,
  /^acknowledged\b[\s\S]{0,60}\bonline\b/i,
  /^[\w-]+\s+online\b/i,
  /\bonline\.?\s+ready\b/i,
  /\b(is|am)\s+online\s+(as|for)\b/i,
  /\bwill\s+(coordinate|maintain|focus|act|draft)\b/i,
];

function recordOf(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

function textFromValue(value: unknown): string {
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) return "";
    try {
      return textFromValue(JSON.parse(trimmed)) || trimmed;
    } catch {
      return trimmed;
    }
  }
  if (Array.isArray(value)) {
    return value.map(textFromValue).filter(Boolean).join(" ").trim();
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const direct =
      record.summary ?? record.message ?? record.text ?? record.body ?? record.reply ??
      record.result ?? record.content ?? record.subject ?? record.request_subject ??
      record.prompt ?? record.description ?? record.token;
    const text = textFromValue(direct);
    if (text) return text;
  }
  return "";
}

function truncate(value: string, max = 110): string {
  const normalized = value
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (normalized.length <= max) return normalized;
  return `${normalized.slice(0, Math.max(0, max - 1)).trimEnd()}...`;
}

function displayName(value: string): string {
  if (!value || value === "_system") return "System";
  return value
    .split(/[-_\s]+/)
    .filter(Boolean)
    .map((part) => `${part.slice(0, 1).toUpperCase()}${part.slice(1)}`)
    .join(" ");
}

function isMeaningfulReply(value: string): boolean {
  const normalized = value.trim().replace(/[.!]+$/g, "").toLowerCase();
  if (!normalized || LOW_VALUE_REPLIES.has(normalized)) return false;
  return !LOW_VALUE_REPLY_PATTERNS.some((pattern) => pattern.test(value.trim()));
}

function lastSegment(value: string): string {
  return value.split("/").pop() || value;
}

function sessionHistoryAssistantReply(frame: ConsoleFrame, data: Record<string, unknown>): string {
  if (frame.sourceKind !== "session_history") {
    return textFromValue(data.result ?? data.text ?? data.content);
  }

  const message = recordOf(data.message);
  const role = typeof message.role === "string" ? message.role : "";
  if (role !== "block_assistant") {
    return textFromValue(data.result ?? data.text ?? data.content);
  }

  const blocks = Array.isArray(message.blocks) ? message.blocks : [];
  const text = blocks
    .map((block) => {
      const record = recordOf(block);
      const blockType = typeof record.block_type === "string"
        ? record.block_type
        : typeof record.type === "string"
          ? record.type
          : "";
      if (blockType !== "text") return "";
      const blockData = recordOf(record.data);
      return textFromValue(blockData.text ?? record.text);
    })
    .filter(Boolean)
    .join(" ")
    .trim();
  return text;
}

function agentFor(frame: ConsoleFrame): string {
  return frame.identity?.trim() || "_system";
}

function peerTarget(args: Record<string, unknown>): string {
  if (typeof args.display_name === "string" && args.display_name.trim()) {
    return lastSegment(args.display_name.trim());
  }
  if (typeof args.to === "string" && args.to.trim()) {
    return lastSegment(args.to.trim());
  }
  return "peer";
}

function isScaffoldRequest(value: string): boolean {
  return /^You have been spawned as\b/i.test(value.trim());
}

function typedSystemNoticeSignal(data: Record<string, unknown>): {
  targets: string[];
  detail: string;
  incoming: boolean;
  kickoff?: MemberKickoffNotice;
} | null {
  const blocks = Array.isArray(data.blocks) ? data.blocks : [];
  const comms = blocks
    .map(recordOf)
    .filter((block) => block.type === "comms");
  if (comms.length === 0) return null;
  // A member-kickoff status is its typed phase, not a peer message.
  const kickoff = comms.map(memberKickoffNotice).find((notice) => notice !== null);
  if (kickoff) return { targets: [kickoff.member], detail: "", incoming: true, kickoff };

  const targets: string[] = [];
  const details: string[] = [];
  let incoming = true;
  for (const block of comms) {
    const peer = recordOf(block.peer);
    const peerLabel = textFromValue(peer.display_name) || textFromValue(peer.id) || "peer";
    targets.push(lastSegment(peerLabel));
    if (block.direction === "outgoing") incoming = false;
    // Pure-scaffold content (the canonical peer transport projection) falls
    // back to the typed summary/intent so signal previews show a parsed
    // intent summary, never the raw envelope.
    // A typed lifecycle notice's content is model-facing notice text.
    const content = block.kind === "lifecycle" ? "" : stripPeerTransportScaffold(textFromValue(block.content));
    const detail = content || textFromValue(block.summary) || textFromValue(block.intent) || textFromValue(block.payload);
    if (detail) details.push(detail);
  }

  return {
    targets,
    detail: details.join(" "),
    incoming,
  };
}

function blobKey(frame: ConsoleFrame): string {
  const data = recordOf(frame.data);
  const image = recordOf(data.image);
  const blobRef = recordOf(image.blob_ref ?? data.blob_ref);
  const blobId = typeof blobRef.blob_id === "string"
    ? blobRef.blob_id
    : typeof data.blob_id === "string"
      ? data.blob_id
      : "";
  const imageId = typeof image.image_id === "string"
    ? image.image_id
    : typeof data.image_id === "string"
      ? data.image_id
      : "";
  return blobId || imageId || frame.interactionId || frame.id;
}

function severityOf(frame: ConsoleFrame): Severity {
  const ev = frame.event;
  if (ev.includes("fail") || ev.includes("error") || ev.includes("crash")) return "critical";
  if (ev === "gating_decision" || ev.includes("warn") || ev.includes("degraded") || ev.includes("retired")) return "warning";
  return "info";
}

function signalFromFrame(frame: ConsoleFrame): Signal | null {
  const data = recordOf(frame.data);
  const severity = severityOf(frame);
  const base = {
    id: frame.id || `${frame.event}:${frame.timestampMs || 0}`,
    severity,
    agent: agentFor(frame),
    at: timeFor(frame.timestampMs),
    raw: frame,
  };

  if (severity === "critical") {
    return {
      ...base,
      label: frame.event === "interaction_failed" ? "Agent turn failed" : frame.event.replace(/_/g, " "),
      detail: truncate(
        describeFailure(data, "")
          || textFromValue(data.error ?? data.reason ?? data.message)
          || "Needs attention",
      ),
    };
  }

  switch (frame.event) {
    case "user_input":
    case "interaction_started": {
      const request = stripPeerTransportScaffold(
        textFromValue(data.content ?? data.text ?? data.prompt),
      );
      if (!request) return null;
      if (isScaffoldRequest(request)) return null;
      return {
        ...base,
        id: `user:${frame.id || frame.interactionId || frame.timestampMs || request}`,
        label: `You asked ${displayName(base.agent)}`,
        detail: truncate(request),
      };
    }
    case "system_notice": {
      const comms = typedSystemNoticeSignal(data);
      if (!comms) return null;
      if (comms.kickoff) {
        const { kickoff } = comms;
        return {
          ...base,
          id: `kickoff:${frame.id || frame.interactionId || frame.timestampMs || kickoff.member}`,
          severity: kickoff.phase === "failed" ? "warning" : base.severity,
          label: `Kickoff ${memberKickoffPhaseLabel(kickoff.phase).toLowerCase()}`,
          detail: truncate(kickoff.role ? `${displayName(kickoff.member)} (${kickoff.role})` : displayName(kickoff.member)),
        };
      }
      const peer = comms.targets.map(displayName).join(", ");
      return {
        ...base,
        id: `comms:${frame.id || frame.interactionId || frame.timestampMs || peer}`,
        label: `${comms.incoming ? "Received from" : "Sent to"} ${peer}`,
        detail: truncate(comms.detail || "Peer comms"),
      };
    }
    case "interaction_complete": {
      const reply = sessionHistoryAssistantReply(frame, data);
      if (!isMeaningfulReply(reply)) return null;
      return {
        ...base,
        label: `${displayName(base.agent)} replied`,
        detail: truncate(reply),
      };
    }
    case "assistant_image":
    case "assistant_image_appended": {
      return {
        ...base,
        id: `image:${blobKey(frame)}`,
        label: `${displayName(base.agent)} generated image`,
        detail: textFromValue(data.prompt ?? recordOf(data.image).prompt ?? recordOf(data.image).alt) || "Generated image attached",
      };
    }
    case "tool_call_requested": {
      const name = typeof data.name === "string" ? data.name : "";
      if (!PEER_TOOLS.has(name)) return null;
      const args = recordOf(data.args);
      const target = peerTarget(args);
      const body = textFromValue(args.body ?? args.params ?? args.result) || textFromValue(args.intent);
      const verb = name === "send_request"
        ? "asked"
        : name === "send_response"
          ? "replied to"
          : "sent to";
      return {
        ...base,
        id: `peer:${frame.id || frame.interactionId || `${target}:${body}`}`,
        label: `${displayName(base.agent)} ${verb} ${displayName(target)}`,
        detail: truncate(body || "Peer comms"),
      };
    }
    case "gating_decision":
      return {
        ...base,
        label: `Gate ${String(data.decision || "decision")}`,
        detail: truncate(textFromValue(data.reason) || "Gating decision recorded"),
      };
    case "member_retired":
      return { ...base, label: "Member retired", detail: truncate(textFromValue(data.reason) || "Lifecycle change") };
    case "state_changed":
      return { ...base, label: `State -> ${String(data.state || data.new_state || "changed")}`, detail: base.agent };
    case "route_changed":
      return { ...base, label: "Route changed", detail: truncate(textFromValue(data.reason) || "Routing updated") };
    default:
      if (frame.event.startsWith("memory.")) {
        return memorySignal(frame, data, base);
      }
      return null;
  }
}

/// Map a `memory.*` frame to a rail signal. Warning-severity events surface
/// operational issues (quarantined writes, taints, budget denials, blocked
/// hygiene, blocked quarantine releases, conflicts); info-severity events
/// surface routine progress. The
/// remaining subtypes (dream start/skip, hygiene proposed/applied/skipped,
/// distill timeouts, non-tainted taint transitions) are dropped from the rail.
function memorySignal(
  frame: ConsoleFrame,
  data: Record<string, unknown>,
  base: Omit<Signal, "label" | "detail">,
): Signal | null {
  const detail = truncate(describeMemoryTimelineEvent(frame.event, data));
  const warning = (label: string): Signal => ({ ...base, severity: "warning", label, detail });
  const info = (label: string): Signal => ({ ...base, severity: "info", label, detail });

  switch (frame.event) {
    case "memory.write.quarantined":
      return warning("Memory write quarantined");
    case "memory.taint.transition":
      return data.kind === "tainted" ? warning("Session memory tainted") : null;
    case "memory.budget.denied":
      return warning("Memory budget denied");
    case "memory.hygiene.blocked":
      return warning("Memory hygiene blocked");
    case "memory.quarantine.release_blocked":
      return warning("Quarantine release blocked");
    case "memory.conflict.signal":
      return warning("Memory conflict");
    case "memory.dream.completed":
      return info("Memory dream completed");
    case "memory.record.promoted":
      return info("Memory record promoted");
    case "memory.harvest.completed":
      return info("Memory harvest completed");
    case "memory.quarantine.verdict":
      return info("Quarantine verdict");
    case "memory.promotion.pending_gate":
      return info("Promotion awaiting gate");
    default:
      // memory.dream.started/skipped, memory.hygiene.proposed/applied/skipped,
      // memory.distill.timed_out, and any other subtype are not shown.
      return null;
  }
}

function groupKeyFor(signal: Signal): string {
  const interactionId = signal.raw.interactionId?.trim();
  if (interactionId) return `interaction:${interactionId}`;
  return `single:${signal.id}`;
}

function semanticSignalKey(signal: Signal): string {
  const canonical = (value: string) => value
    .replace(/\+\d+\s+-\d+\b/g, "")
    .replace(/[\u2018\u2019]/g, "'")
    .replace(/[\u201c\u201d]/g, "\"")
    .replace(/\s+/g, " ")
    .replace(/[.!?\s]+$/g, "")
    .trim()
    .toLowerCase();
  return [
    canonical(signal.agent),
    canonical(signal.label),
    canonical(signal.detail),
  ].join("\u0000");
}

function strongerSeverity(a: Severity, b: Severity): Severity {
  if (a === "critical" || b === "critical") return "critical";
  if (a === "warning" || b === "warning") return "warning";
  return "info";
}

function groupSignals(signals: Signal[]): SignalGroup[] {
  const groups: SignalGroup[] = [];
  const byId = new Map<string, SignalGroup>();
  for (const signal of signals) {
    const key = groupKeyFor(signal);
    const existing = byId.get(key);
    if (existing) {
      const seenItemKeys = new Set(existing.items.map(semanticSignalKey));
      if (!seenItemKeys.has(semanticSignalKey(signal))) {
        existing.items.push(signal);
      }
      existing.severity = strongerSeverity(existing.severity, signal.severity);
      existing.title = titleForGroup(existing.items);
      existing.detail = detailForGroup(existing.items);
      existing.agent = existing.items[0]?.agent || signal.agent;
      existing.at = existing.items[0]?.at || signal.at;
      continue;
    }
    const group = {
      id: key,
      severity: signal.severity,
      title: titleForGroup([signal]),
      detail: detailForGroup([signal]),
      agent: signal.agent,
      at: signal.at,
      items: [signal],
    };
    byId.set(key, group);
    groups.push(group);
  }
  return groups;
}

function titleForGroup(items: Signal[]): string {
  if (items.length === 1) return items[0].label;
  const hasUser = items.some((item) => item.raw.event === "user_input" || item.raw.event === "interaction_started");
  const peerCount = items.filter((item) => item.raw.event === "tool_call_requested").length;
  const replyCount = items.filter((item) => item.raw.event === "interaction_complete").length;
  if (hasUser && (peerCount > 0 || replyCount > 0)) return "Turn activity";
  if (peerCount > 1) return "Peer conversation";
  return `${items.length} related events`;
}

function detailForGroup(items: Signal[]): string {
  if (items.length === 1) return items[0].detail;
  const newestReply = items.find((item) => item.raw.event === "interaction_complete");
  const userRequest = items.find((item) => item.raw.event === "user_input" || item.raw.event === "interaction_started");
  return newestReply?.detail || userRequest?.detail || items[0]?.detail || "";
}

function timeFor(tsMs?: number): string {
  if (!tsMs) return "--";
  const diff = Date.now() - tsMs;
  if (diff < 60_000) return `${Math.max(1, Math.floor(diff / 1000))}s`;
  if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m`;
  return `${Math.floor(diff / 3_600_000)}h`;
}

export function buildSignalGroupsForTest(frames: ConsoleFrame[]): SignalGroup[] {
  const seen = new Set<string>();
  const seenSemantic = new Set<string>();
  const next: Signal[] = [];
  for (const frame of frames.slice(0, 260)) {
    const signal = signalFromFrame(frame);
    if (!signal) continue;
    if (seen.has(signal.id)) continue;
    seen.add(signal.id);
    const semanticKey = semanticSignalKey(signal);
    if (seenSemantic.has(semanticKey)) continue;
    seenSemantic.add(semanticKey);
    next.push(signal);
    if (next.length >= 80) break;
  }
  return groupSignals(next);
}

/// Memoised: the app root re-renders on every coalesced frame flush and on
/// each debounced draft publish, and the props here are stable references
/// (see ConsoleApp), so unchanged input skips the whole subtree.
export const SignalsRail = React.memo(function SignalsRail({
  frames,
  collapsed,
  filterPresets,
  activePresetId,
  emptyText,
  watchedIdentities,
  onPresetChange,
  onSelect,
}: SignalsRailProps): React.JSX.Element {
  countRender("SignalsRail");
  const presets = React.useMemo(() => {
    const configured = (filterPresets || []).filter((preset) => preset.id && preset.label);
    return configured.length > 0 ? configured : DEFAULT_FILTER_PRESETS;
  }, [filterPresets]);
  const [filter, setFilter] = React.useState<string>(activePresetId || presets[0]?.id || "all");
  const [expandedGroups, setExpandedGroups] = React.useState<Set<string>>(() => new Set());

  React.useEffect(() => {
    if (activePresetId && presets.some((preset) => preset.id === activePresetId)) {
      setFilter(activePresetId);
    }
  }, [activePresetId, presets]);

  const groups: SignalGroup[] = React.useMemo(() => {
    return buildSignalGroupsForTest(frames);
  }, [frames]);

  function groupMatchesPreset(group: SignalGroup, preset: ConsoleRailFilterPresetConfig): boolean {
    if (preset.watchedOnly) {
      const watched = watchedIdentities || new Set<string>();
      const isWatched = group.items.some((item) => {
        const identity = item.raw.identity || "";
        return identity && watched.has(identity);
      });
      if (!isWatched) return false;
    }
    const alertLevels = new Set((preset.alertLevels || []).map((level) => level.toLowerCase()));
    if (alertLevels.size > 0 && !alertLevels.has(group.severity)) return false;
    return true;
  }

  const activePreset = presets.find((preset) => preset.id === filter) || presets[0] || DEFAULT_FILTER_PRESETS[0];
  const counts = React.useMemo(() => {
    return new Map(presets.map((preset) => [
      preset.id,
      groups.filter((group) => groupMatchesPreset(group, preset)).length,
    ]));
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groups, presets, watchedIdentities]);

  const shown = groups.filter((group) => groupMatchesPreset(group, activePreset));

  const recent15m = groups.filter((s) => (Date.now() - (s.items[0]?.raw.timestampMs || 0)) < 15 * 60 * 1000).length;

  function toggleGroup(group: SignalGroup): void {
    if (group.items.length <= 1) {
      onSelect?.(group.items[0].raw);
      return;
    }
    setExpandedGroups((prev) => {
      const next = new Set(prev);
      if (next.has(group.id)) next.delete(group.id);
      else next.add(group.id);
      return next;
    });
  }

  if (collapsed) {
    return (
      <aside
        className="rail rail--collapsed"
        data-collapsed="true"
        data-testid="signals-rail"
      >
        <i className="rail__grip" aria-hidden="true" />
      </aside>
    );
  }

  return (
    <aside className="rail" data-testid="signals-rail">
      <div className="rail__head">
        <span className="rail__title">Signals</span>
        <span className="rail__sub">{recent15m} in 15m</span>
      </div>
      <div className="rail__filters">
        {presets.map((preset) => (
          <button
            key={preset.id}
            className={`rail__filter ${filter === preset.id ? "is-active" : ""}`}
            onClick={() => {
              setFilter(preset.id);
              onPresetChange?.(preset.id);
            }}
            data-testid={`signals-filter:${preset.id}`}
          >
            {preset.label} <span className="rail__filter-count">{counts.get(preset.id) || 0}</span>
          </button>
        ))}
      </div>
      <div className="rail__list">
        {shown.length === 0 && (
          <div className="rail__empty">
            {emptyText || "No meaningful signals yet."}
          </div>
        )}
        {shown.map((s) => {
          const expanded = expandedGroups.has(s.id);
          return (
          <div
            key={s.id}
            className="signal"
            data-sev={s.severity}
            data-testid={`signal:${s.id}`}
            data-expanded={expanded ? "true" : "false"}
            onClick={() => toggleGroup(s)}
            role="button"
            tabIndex={0}
          >
            <span className="signal__bar" />
            <span className="signal__body">
              <span className="signal__label">
                {s.items.length > 1 && <span className="signal__chevron">{expanded ? "▾" : "▸"}</span>}
                {s.title}
                {s.items.length > 1 && <span className="signal__count">{s.items.length}</span>}
              </span>
              <span className="signal__detail">{s.detail}</span>
              <span className="signal__agent">{s.agent}</span>
              {s.items.length === 1 &&
                s.items[0].raw.event.startsWith("memory.") &&
                onSelect ? (
                <button
                  type="button"
                  className="signal__memory-pivot"
                  data-testid="signal-memory-pivot"
                  onClick={(event) => {
                    event.stopPropagation();
                    onSelect(s.items[0].raw);
                  }}
                >
                  state here
                </button>
              ) : null}
              {s.items.length > 1 && expanded && (
                <span className="signal__events">
                  {s.items.map((item) => (
                    <button
                      key={item.id}
                      className="signal__event"
                      type="button"
                      onClick={(event) => {
                        event.stopPropagation();
                        onSelect?.(item.raw);
                      }}
                    >
                      <span className="signal__event-label">{item.label}</span>
                      <span className="signal__event-detail">{item.detail}</span>
                    </button>
                  ))}
                </span>
              )}
            </span>
            <span className="signal__meta">
              <span className="signal__time">{s.at}</span>
            </span>
          </div>
        );})}
      </div>
    </aside>
  );
});
