import { operationFeedbackFromFrame } from "./operation-feedback";
import { mcpAppEntryFromFrame } from "./mcp-app-projection";
import { assistantPresentationEntries, conversationPresentationRows } from "./assistant-presentation";
import { userMessageRenderKey } from "./user-message-identity";
import { realtimeMessageOrigin, isRealtimeHistoryMessage } from "./realtime-message-identity";
import { assistantMessageKey, assistantMessageRenderKey, hasAssistantMessageIdCarrier } from "./assistant-message-identity";
import { canonicalAssistantToolCounterparts, reconcileAssistantMessageFrames } from "./assistant-message-projection";
import { loadedTimelineStartMs, reconcileAssistantHistoryPositions, reconcileRuntimeAppendFrames, runtimeAppendNoticeKey } from "./runtime-append-projection";
import { settledToolCompletion, toolCompletionFromFrame, unknownToolCompletion, type ToolCompletionEvidence } from "./tool-completion";
import { parseConsoleContextMessage } from "./context-record";
import { memberKickoffNotice, memberKickoffPhaseLabel } from "./member-kickoff";
import {
  decodeMemberAlias,
  entryOriginFromFrameData,
  runtimeEventFromFrame,
  runtimeEventText,
} from "./transcript-source";
import type {
  ConsoleActivityPulseItem,
  ConsoleActivityRailViewState,
} from "./activity";
import type { ActivityFilterPreset, ResponsePhase, RoutingSectionView } from "./control-plane";
import {
  normalizeRoutingSectionView,
  normalizeSidebarWatchFields,
} from "./control-plane";
import type { ConsoleDockTarget } from "./dock";
import type {
  ConversationEmptySuggestion,
  ConversationIdentity,
  ConversationTimelineEntry,
  ConversationViewState,
} from "./conversation";
import {
  groupConversationTimelineEntries,
} from "./conversation";
import type {
  ConsoleSidebarMetaTone,
  ConsoleSidebarViewState,
} from "./sidebar";
import {
  buildConversationMarkdownBlocks,
  parseConversationRichBlocks,
  parseStreamingConversationRichBlocks,
} from "./rich-content";
import type {
  ConversationRichBlock,
  ConversationRichImageBlock,
  ConversationTextMode,
  ConversationRichToolCallBlock,
} from "./rich-content";
import type {
  ConsoleAgent,
  ConsoleFrame,
} from "./runtime-types";

function messageTextBlocks(source: string, textMode: ConversationTextMode = "legacy", streaming = false): ConversationRichBlock[] {
  if (textMode === "markdown") return buildConversationMarkdownBlocks(source, { streaming });
  return streaming
    ? parseStreamingConversationRichBlocks(source, { displayNormalization: false })
    : parseConversationRichBlocks(source, { displayNormalization: false });
}

export type MetaDockTarget =
  | AgentChatTarget
  | IdentityInspectTarget
  | RoutingPanelTarget
  | GatingPanelTarget
  | TopologyPanelTarget
  | HealthPanelTarget
  | TimelinePanelTarget
  | RosterPanelTarget
  | GatesPanelTarget
  | LogsPanelTarget
  | MemoryPanelTarget
  | WorkGraphPanelTarget;

export interface AgentChatTarget extends ConsoleDockTarget {
  kind: "agent-chat";
  addressingMode: "identity";
  memberId: string;
  identity: string;
}

export interface IdentityInspectTarget extends ConsoleDockTarget {
  kind: "identity-inspect";
  identity: string;
  memberId: string;
}

export interface RoutingPanelTarget extends ConsoleDockTarget {
  kind: "routing";
}

export interface GatingPanelTarget extends ConsoleDockTarget {
  kind: "gating";
}

export interface TopologyPanelTarget extends ConsoleDockTarget {
  kind: "topology";
}

export interface HealthPanelTarget extends ConsoleDockTarget {
  kind: "health";
}

export interface TimelinePanelTarget extends ConsoleDockTarget {
  kind: "timeline";
}

export interface RosterPanelTarget extends ConsoleDockTarget {
  kind: "roster";
}

export interface GatesPanelTarget extends ConsoleDockTarget {
  kind: "gates";
}

export interface LogsPanelTarget extends ConsoleDockTarget {
  kind: "logs";
}

export interface MemoryPanelTarget extends ConsoleDockTarget {
  kind: "memory";
}

export interface WorkGraphPanelTarget extends ConsoleDockTarget {
  kind: "workgraph";
}

export function buildPanelConversationKey(
  panelId: string,
  target: (Pick<ConsoleDockTarget, "id" | "kind"> & Partial<Pick<AgentChatTarget, "identity" | "memberId" | "addressingMode">>) | null,
): string {
  if (!target) {
    return `panel:${panelId}:none`;
  }
  if (target.kind !== "agent-chat") {
    return `panel:${panelId}:${target.kind}:${target.id}`;
  }
  const targetKey = target.identity || target.memberId || target.id;
  return `panel:${panelId}:${target.kind}:${targetKey}`;
}

export interface OptimisticUserMessage {
  interactionId: string;
  entry: ConversationTimelineEntry;
  sentAtMs: number;
  objectUrls?: string[];
}

export function optimisticUserMessageForPanel(
  optimisticByPanelKey: Record<string, OptimisticUserMessage>,
  panelKey: string,
  identity: string,
): OptimisticUserMessage | null {
  const direct = optimisticByPanelKey[panelKey];
  if (direct) return direct;
  const identitySuffix = `:agent-chat:${identity}`;
  let latest: OptimisticUserMessage | null = null;
  for (const [key, optimistic] of Object.entries(optimisticByPanelKey)) {
    if (!key.endsWith(identitySuffix)) continue;
    if (!latest || optimistic.sentAtMs > latest.sentAtMs) latest = optimistic;
  }
  return latest;
}

export function buildDockTarget(agent: ConsoleAgent): AgentChatTarget {
  const subtitle = [agent.role, agent.kind].filter(Boolean).join(" \u00b7 ") || undefined;
  const identity = typeof agent.identity === "string" && agent.identity.trim()
    ? agent.identity.trim()
    : agent.member_id;
  return {
    id: agent.member_id,
    kind: "agent-chat",
    addressingMode: "identity",
    memberId: agent.member_id,
    identity,
    title: agent.label,
    subtitle,
    iconName: "i-team",
  };
}

export function buildInspectTarget(agent: ConsoleAgent): IdentityInspectTarget {
  return {
    id: `inspect:${agent.identity || agent.member_id}`,
    kind: "identity-inspect",
    identity: agent.identity || agent.member_id,
    memberId: agent.member_id,
    title: `${agent.label} Details`,
    subtitle: agent.identity || agent.member_id,
    iconName: "i-terminal",
  };
}

export type ControlTargetKind =
  | "routing" | "gating" | "topology" | "health"
  | "timeline" | "roster" | "gates" | "logs" | "memory" | "workgraph";

export function buildControlTarget(kind: ControlTargetKind): MetaDockTarget {
  switch (kind) {
    case "routing":
      return { id: "routing", kind, title: "Routing", subtitle: "Routes and delivery history", iconName: "i-swap" };
    case "gating":
      return { id: "gating", kind, title: "Approvals", subtitle: "Pending approvals, audit, and policies", iconName: "i-bolt" };
    case "topology":
      return { id: "topology", kind, title: "Topology", subtitle: "Identity connectivity", iconName: "i-team" };
    case "health":
      return { id: "health", kind, title: "Health", subtitle: "Runtime and identity health", iconName: "i-gear" };
    case "timeline":
      return { id: "timeline", kind, title: "Today", subtitle: "Chronological events", iconName: "i-clock" };
    case "roster":
      return { id: "roster", kind, title: "Roster", subtitle: "All agents", iconName: "i-team" };
    case "gates":
      return { id: "gating", kind: "gating", title: "Approvals", subtitle: "Pending approvals, audit, and policies", iconName: "i-bolt" };
    case "logs":
      return { id: "logs", kind, title: "Logs", subtitle: "Event stream", iconName: "i-terminal" };
    case "memory":
      return { id: "memory", kind, title: "Memory", subtitle: "Records, quarantine, dreams", iconName: "i-archive" };
    case "workgraph":
      return { id: "workgraph", kind, title: "WorkGraph", subtitle: "Goals, work items, and attention", iconName: "i-cube" };
    default:
      return { id: "health", kind: "health", title: "Health" };
  }
}

function agentGroupKey(agent: ConsoleAgent): string {
  return agent.group?.trim() || agent.role?.trim() || agent.kind?.trim() || "Agents";
}

function agentStateTone(state: string | undefined): ConsoleSidebarMetaTone {
  switch (state) {
    case "running": return "accent";
    case "active": return "positive";
    case "idle": return "muted";
    case "error": return "negative";
    default: return "muted";
  }
}

function sectionIconForGroup(group: string): string | null {
  const lower = group.toLowerCase();
  if (lower.includes("coordinator") || lower.includes("system")) return "i-bolt";
  if (lower.includes("domain") || lower.includes("specialist")) return "i-cube";
  if (lower.includes("internal") || lower.includes("infra")) return "i-gear";
  if (lower.includes("personal") || lower.includes("identity")) return "i-team";
  return "i-folder";
}

/**
 * Durable pin identity for an agent. Prefers the stable identity (or the
 * configured `labels.agent_identity`) so pins survive respawns, falling back to
 * the volatile `member_id` only when no durable identity exists.
 */
export function sidebarAgentPinId(agent: ConsoleAgent): string {
  return agent.identity?.trim()
    || agent.labels?.agent_identity?.trim()
    || agent.member_id.trim();
}

export function isAgentPinned(agent: ConsoleAgent, pinnedAgentIds: Set<string> | undefined): boolean {
  if (!pinnedAgentIds) return false;
  return pinnedAgentIds.has(sidebarAgentPinId(agent)) || pinnedAgentIds.has(agent.member_id);
}

export function buildSidebarViewState(args: {
  agents: ConsoleAgent[];
  selectedMemberId: string;
  pinnedAgentIds?: Set<string>;
  sortMode?: "group" | "alpha" | "status";
}): ConsoleSidebarViewState {
  const { agents, selectedMemberId, pinnedAgentIds = new Set(), sortMode = "group" } = args;

  const sorted = [...agents].sort((a, b) => {
    const aPinned = isAgentPinned(a, pinnedAgentIds) ? 0 : 1;
    const bPinned = isAgentPinned(b, pinnedAgentIds) ? 0 : 1;
    if (aPinned !== bPinned) return aPinned - bPinned;

    if (sortMode === "alpha") return a.label.localeCompare(b.label);
    if (sortMode === "status") {
      const stateOrder = (s: string | undefined) => s === "running" ? 0 : s === "active" ? 1 : 2;
      const diff = stateOrder(a.state) - stateOrder(b.state);
      if (diff !== 0) return diff;
    }
    return a.label.localeCompare(b.label);
  });

  const grouped = new Map<string, ConsoleAgent[]>();
  for (const agent of sorted) {
    const key = agentGroupKey(agent);
    const bucket = grouped.get(key) || [];
    bucket.push(agent);
    grouped.set(key, bucket);
  }

  const sections = Array.from(grouped.entries()).map(([group, members]) => ({
    id: group,
    title: group,
    iconName: sectionIconForGroup(group),
    meta: [{ id: "count", label: `${members.length}` }] as Array<{ id: string; label: string; tone?: ConsoleSidebarMetaTone }>,
    items: members.map((agent) => {
      const isAddressable = agent.addressable || agent.affordances?.can_send_message;
      const isPinned = isAgentPinned(agent, pinnedAgentIds);
      const watchFields = normalizeSidebarWatchFields(agent);
      return {
        id: agent.member_id,
        title: agent.label,
        subtitle: agent.identity || agent.member_id,
        selected: agent.member_id === selectedMemberId,
        pinned: isPinned,
        disabled: !isAddressable,
        ...watchFields,
        meta: [
          ...(agent.state ? [{ id: "state", label: agent.state, tone: agentStateTone(agent.state) }] : []),
          ...(agent.response_phase ? [{ id: "phase", label: agent.response_phase, tone: "accent" as const }] : []),
        ],
        actions: [
          {
            id: "inspect_identity",
            label: "Open roster details",
            iconName: "i-terminal",
          },
          {
            id: "toggle_pin",
            label: isPinned ? "Unpin agent" : "Pin agent",
            iconName: "i-pin",
            active: isPinned,
          },
        ],
      };
    }),
  }));

  return {
    blocks: [
      {
        id: "controls",
        kind: "action_strip" as const,
        actions: [
          { id: "open_routing", label: "Routing", iconName: "i-swap" },
          { id: "open_gating", label: "Gating", iconName: "i-bolt" },
          { id: "open_topology", label: "Topology", iconName: "i-team" },
          { id: "open_health", label: "Health", iconName: "i-gear" },
        ],
      },
      {
        id: "agents",
        kind: "list" as const,
        title: "Agents",
        actions: [
          { id: "spawn_agent", label: "Spawn agent", iconName: "i-plus" },
          { id: "filter_sort", label: "Sort & filter", iconName: "i-sliders" },
        ],
        sections,
      },
    ],
  };
}

export function buildRoutingSectionView(args: {
  routesResponse: unknown;
  historyResponse: unknown;
}): RoutingSectionView {
  const routesRecord = typeof args.routesResponse === "object" && args.routesResponse !== null
    ? args.routesResponse as Record<string, unknown>
    : {};
  const historyRecord = typeof args.historyResponse === "object" && args.historyResponse !== null
    ? args.historyResponse as Record<string, unknown>
    : {};
  const normalized = normalizeRoutingSectionView({
    routes: Array.isArray(routesRecord.routes) ? routesRecord.routes : [],
    deliveries: Array.isArray(historyRecord.deliveries) ? historyRecord.deliveries : [],
  });

  return normalized ?? { routes: [], deliveries: [] };
}

const USER_IDENTITY: ConversationIdentity = {
  id: "user",
  label: "You",
  role: "user",
};

function agentIdentity(agent: ConsoleAgent | null): ConversationIdentity {
  return {
    id: agent?.member_id || "agent",
    label: agent?.label || "Agent",
    role: "assistant",
  };
}

const SYSTEM_IDENTITY: ConversationIdentity = {
  id: "system",
  label: "System",
  role: "system",
  presentation: "system",
  showLabel: true,
};

const COMMS_IDENTITY: ConversationIdentity = {
  id: "comms",
  label: "",
  role: "system",
  showLabel: false,
};

function summarizeFrameData(data: unknown): string {
  if (typeof data === "string") {
    const trimmed = data.trim();
    if ((trimmed.startsWith("{") && trimmed.endsWith("}")) || (trimmed.startsWith("[") && trimmed.endsWith("]"))) {
      try {
        return summarizeFrameData(JSON.parse(trimmed));
      } catch {
        return data;
      }
    }
    return data;
  }
  if (typeof data === "object" && data !== null) {
    const record = data as Record<string, unknown>;
    if (typeof record.delta === "string") return record.delta;
    if (typeof record.text === "string" && record.text.trim()) return record.text;
    // For canonical text-bearing frames (`interaction_complete`,
    // `tool_execution_completed`, etc.) `record.result` IS the
    // canonical text. Return it verbatim — including the empty
    // string. Pre-fix the `.trim()` guard skipped empty results and
    // we fell through to `JSON.stringify(record)`, which dumped the
    // whole envelope (`session_id`, `type`, `usage`, ...) into the
    // assistant's chat bubble.
    if (typeof record.result === "string") return record.result;
    if (typeof record.message === "string" && record.message.trim()) return record.message;
    if (typeof record.error === "string" && record.error.trim()) return record.error;
    if (typeof record.reason === "string" && record.reason.trim()) return record.reason;
    if (typeof record.kind === "string" && typeof record.event_type === "string") return "";
    return JSON.stringify(record);
  }
  return String(data ?? "");
}

// ── Memory timeline event formatting ──────────────────────────────────────
//
// The runtime emits a family of `memory.*` timeline events (dream cycles,
// promotions, quarantine verdicts, taint transitions, hygiene ops, ...). This
// pure helper turns each `{event, data}` pair into a single clean human line.
// Unknown `memory.*` subtypes get a humanized event name plus a best-effort
// short reason — NEVER raw JSON. Exported so it is unit-testable and reused by
// the SignalsRail.
function memoryString(data: Record<string, unknown>, key: string): string {
  const value = data[key];
  return typeof value === "string" && value.trim() ? value.trim() : "";
}

function memoryNumber(data: Record<string, unknown>, key: string): number | null {
  const value = data[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function memoryScopeLabel(data: Record<string, unknown>): string {
  const kind = memoryString(data, "scope_kind");
  const key = memoryString(data, "scope_key");
  if (kind && key) return `${kind}:${key}`;
  return kind || key || "";
}

function humanizeMemoryEvent(event: string): string {
  const suffix = event.startsWith("memory.") ? event.slice("memory.".length) : event;
  const words = suffix.split(/[._]/).filter(Boolean);
  if (words.length === 0) return "Memory event";
  return `Memory ${words.join(" ")}`;
}

export function describeMemoryTimelineEvent(
  event: string,
  data: Record<string, unknown>,
): string {
  switch (event) {
    case "memory.dream.started":
      return `Dream started${memoryString(data, "run_id") ? ` (${memoryString(data, "run_id")})` : ""}`;
    case "memory.dream.completed": {
      const ops = memoryNumber(data, "ops_committed");
      const detail = memoryString(data, "detail");
      const opsText = ops !== null ? `${ops} op${ops === 1 ? "" : "s"} committed` : "completed";
      return `Dream ${opsText}${detail ? ` — ${detail}` : ""}`;
    }
    case "memory.dream.skipped":
      return `Dream skipped${memoryString(data, "reason") ? ` — ${memoryString(data, "reason")}` : ""}`;
    case "memory.record.promoted": {
      const scope = memoryScopeLabel(data);
      const gated = data.gated === true ? " (gated)" : "";
      return `Record promoted${scope ? ` to ${scope}` : ""}${gated}`;
    }
    case "memory.quarantine.verdict": {
      const verdict = memoryString(data, "verdict") || "decided";
      const rationale = memoryString(data, "rationale");
      return `Quarantine verdict: ${verdict}${rationale ? ` — ${rationale}` : ""}`;
    }
    case "memory.quarantine.release_blocked": {
      const verdict = memoryString(data, "verdict");
      const action = verdict === "promote_pending_gate" ? "promotion" : verdict || "release";
      const record = memoryString(data, "record_id");
      const cls = memoryString(data, "class");
      return `Quarantine ${action} blocked${record ? ` for ${record}` : ""}${cls ? ` — matches secret pattern ${cls}` : ""}`;
    }
    case "memory.conflict.signal": {
      const entity = memoryString(data, "entity");
      const topic = memoryString(data, "topic");
      const subject = [entity, topic].filter(Boolean).join(" / ");
      const reason = memoryString(data, "reason");
      return `Conflict signal${subject ? ` on ${subject}` : ""}${reason ? ` — ${reason}` : ""}`;
    }
    case "memory.write.quarantined": {
      const author = memoryString(data, "author");
      const reason = memoryString(data, "reason");
      return `Write quarantined${author ? ` from ${author}` : ""}${reason ? ` — ${reason}` : ""}`;
    }
    case "memory.taint.transition": {
      const kind = memoryString(data, "kind") || "changed";
      const label =
        kind === "tainted"
          ? "Session tainted"
          : kind === "reset_boundary"
            ? "Reset boundary"
            : kind === "rotated_clean"
              ? "Rotated clean"
              : `Taint ${kind}`;
      const source = memoryString(data, "source");
      const session = memoryString(data, "session_key");
      const context = [session, source].filter(Boolean).join(" · ");
      return `${label}${context ? ` (${context})` : ""}`;
    }
    case "memory.budget.denied": {
      const stage = memoryString(data, "stage");
      const reason = memoryString(data, "reason");
      return `Budget denied${stage ? ` at ${stage}` : ""}${reason ? ` — ${reason}` : ""}`;
    }
    case "memory.promotion.pending_gate": {
      const scope = memoryScopeLabel(data);
      return `Promotion awaiting gate${scope ? ` for ${scope}` : ""}`;
    }
    case "memory.harvest.completed": {
      const promoted = memoryNumber(data, "promoted");
      const tombstoned = memoryNumber(data, "tombstoned");
      const parts: string[] = [];
      if (promoted !== null) parts.push(`${promoted} promoted`);
      if (tombstoned !== null) parts.push(`${tombstoned} tombstoned`);
      const identity = memoryString(data, "identity");
      return `Harvest completed${parts.length ? ` — ${parts.join(", ")}` : ""}${identity ? ` (${identity})` : ""}`;
    }
    case "memory.distill.timed_out": {
      const cause = memoryString(data, "cause");
      const session = memoryString(data, "session_key");
      return `Distill timed out${cause ? ` — ${cause}` : ""}${session ? ` (${session})` : ""}`;
    }
    case "memory.hygiene.proposed":
    case "memory.hygiene.applied":
    case "memory.hygiene.blocked":
    case "memory.hygiene.skipped": {
      const phase = event.slice("memory.hygiene.".length);
      const cause = memoryString(data, "cause");
      const ops = memoryNumber(data, "ops");
      const reason = memoryString(data, "reason");
      const detail =
        reason ||
        (ops !== null ? `${ops} op${ops === 1 ? "" : "s"}` : "") ||
        cause;
      return `Hygiene ${phase}${detail ? ` — ${detail}` : ""}`;
    }
    default: {
      // Unknown memory.* subtype: humanized name + best-effort short reason.
      const reason =
        memoryString(data, "reason") ||
        memoryString(data, "detail") ||
        memoryString(data, "cause") ||
        memoryString(data, "verdict");
      return `${humanizeMemoryEvent(event)}${reason ? ` — ${reason}` : ""}`;
    }
  }
}

function isSteerDeliveryTerminalFrame(frame: Pick<ConsoleFrame, "event" | "data">): boolean {
  if (frame.event !== "interaction_complete") return false;
  if (!frame.data || typeof frame.data !== "object") return false;
  const record = frame.data as Record<string, unknown>;
  return record.reason === "steer_delivered";
}

function eventSortRank(event: string | undefined): number {
  switch (event) {
    case "user_input":
    case "interaction_started":
    case "run_started":
      return 0;
    case "tool_call_requested":
    case "tool_call":
    case "tool_execution_started":
      return 20;
    case "tool_result_received":
    case "tool_execution_completed":
      return 30;
    case "assistant_image":
    case "assistant_image_appended":
      return 35;
    // Reasoning ("thinking") precedes the answer text it leads to; rank it before
    // text so equal-timestamp (or timestamp-less) reasoning frames don't sort
    // after the answer.
    case "reasoning_delta":
    case "reasoning_complete":
      return 38;
    case "text_delta":
      return 40;
    case "text_complete":
      return 45;
    case "interaction_complete":
    case "interaction_failed":
    case "run_completed":
    case "run_failed":
      return 90;
    default:
      return 50;
  }
}

function isInteractionStartEvent(event: string | undefined): boolean {
  return event === "user_input" || event === "interaction_started" || event === "run_started";
}

function cursorSeq(cursor: string | undefined): number | null {
  if (!cursor) return null;
  const match = /^console:(\d+)$/.exec(cursor);
  if (!match) return null;
  const parsed = Number(match[1]);
  return Number.isFinite(parsed) ? parsed : null;
}

// Runtime sequence is session-scoped; console cursor is a store observation.
// Use either before event-kind fallback only when the two owners are comparable.
function transcriptSourceOrder(left: ConsoleFrame, right: ConsoleFrame): number | null {
  if (ownerContextsConflict(left, right)) return null;
  const sameSession = Boolean(left.runtimeKey && left.runtimeKey === right.runtimeKey
    && left.sessionId && left.sessionId === right.sessionId);
  if (sameSession && left.sourceKind === "console_event" && right.sourceKind === "console_event") {
    const sequence = (frame: ConsoleFrame): number | null => {
      const value = frame.data && typeof frame.data === "object"
        ? (frame.data as Record<string, unknown>).source_sequence : undefined;
      return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : null;
    };
    const a = sequence(left), b = sequence(right);
    if (a !== null && b !== null && a !== b) return a - b;
  }
  const sameInteraction = Boolean(left.interactionId?.trim()
    && left.interactionId === right.interactionId);
  const sameRun = Boolean(left.runId?.trim() && left.runId === right.runId);
  if (!sameSession && !sameInteraction && !sameRun) return null;
  // Backfilled history has a later observation cursor than its live twin.
  // That cursor cannot supply assistant/tool order in the source transcript.
  if (left.sourceKind === "session_history" || right.sourceKind === "session_history") return null;
  const a = cursorSeq(left.cursor), b = cursorSeq(right.cursor);
  return a !== null && b !== null && a !== b ? a - b : null;
}

function sortFramesForTranscript(frames: ConsoleFrame[]): ConsoleFrame[] {
  const interactionStartMs = new Map<string, number>();
  for (const frame of frames) {
    const interactionId = frame.interactionId?.trim();
    const timestampMs = typeof frame.timestampMs === "number" ? frame.timestampMs : Number.MAX_SAFE_INTEGER;
    if (!interactionId) continue;
    const current = interactionStartMs.get(interactionId);
    if (current === undefined || timestampMs < current) {
      interactionStartMs.set(interactionId, timestampMs);
    }
  }

  const transcriptGroupTimestamp = (frame: ConsoleFrame): number => {
    const interactionId = frame.interactionId?.trim() || "";
    const ownTimestamp =
      typeof frame.timestampMs === "number"
        ? frame.timestampMs
        : Number.MAX_SAFE_INTEGER;
    if (!interactionId) return ownTimestamp;
    return interactionStartMs.get(interactionId) ?? ownTimestamp;
  };

  return frames
    .map((frame, index) => ({ frame, index }))
    .sort((left, right) => {
      const leftInteraction = left.frame.interactionId?.trim() || "";
      const rightInteraction = right.frame.interactionId?.trim() || "";
      const leftGroupTs = transcriptGroupTimestamp(left.frame);
      const rightGroupTs = transcriptGroupTimestamp(right.frame);
      if (leftInteraction && rightInteraction && leftInteraction === rightInteraction) {
        const leftStarts = isInteractionStartEvent(left.frame.event);
        const rightStarts = isInteractionStartEvent(right.frame.event);
        if (leftStarts !== rightStarts) {
          return leftStarts ? -1 : 1;
        }
      }
      // OWN timestamp first; the interaction's group timestamp is only a
      // FALLBACK for frames carrying none. See the twin in
      // `console/src/lib/adapters.ts` for the full rationale.
      //
      // Comparing group START times first returned before own timestamps were
      // ever consulted, so frames in different interactions never had their own
      // timestamps compared. That is right while interactions do not overlap
      // and wrong the moment they do - which is precisely what a mid-turn steer
      // creates, since a steer is minted as its own interaction while the
      // running turn's frames keep the original id. Everything the turn
      // produced after the steer rendered above it, permanently.
      //
      // The fallback must stay: reasoning frames often arrive with no
      // timestamp, and a MAX_SAFE_INTEGER default would render "thinking" at
      // the end of the turn.
      const leftTs = typeof left.frame.timestampMs === "number" ? left.frame.timestampMs : leftGroupTs;
      const rightTs = typeof right.frame.timestampMs === "number" ? right.frame.timestampMs : rightGroupTs;
      if (leftTs !== rightTs) {
        return leftTs - rightTs;
      }
      const sourceOrder = transcriptSourceOrder(left.frame, right.frame);
      if (sourceOrder !== null) return sourceOrder;
      if (leftInteraction && rightInteraction && leftInteraction === rightInteraction) {
        const leftRank = eventSortRank(left.frame.event);
        const rightRank = eventSortRank(right.frame.event);
        if (leftRank !== rightRank) {
          return leftRank - rightRank;
        }
      }
      const leftCursor = cursorSeq(left.frame.cursor);
      const rightCursor = cursorSeq(right.frame.cursor);
      if (leftCursor !== null && rightCursor !== null && leftCursor !== rightCursor) {
        return leftCursor - rightCursor;
      }
      return left.index - right.index;
    })
    .map(({ frame }) => frame);
}

const HIDDEN_EVENTS = new Set([
  "subscribed",
  "run_started",
  "run_completed",
  "turn_started",
  "turn_completed",
  "text_complete",
  "reasoning_delta",
  "reasoning_complete",
  "interaction_started",
  "frame_updated",
  "snapshot_complete",
  "snapshot_started",
  // The owner's access revision signal: it re-reads access views and is
  // never a transcript or activity entry.
  "access_changed",
  "run_failed",
  "keep-alive",
  "server_tool_content",
  "tool_config_changed",
  "tool_scope_changed",
]);

function appendDistinctText(parts: string[], value: string): void {
  const text = value.trim();
  if (!text) return;
  const comparable = normalizeComparableText(text);
  if (parts.some((part) => normalizeComparableText(part) === comparable)) return;
  parts.push(text);
}

function textFromReasoningValue(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (Array.isArray(value)) {
    return value
      .map((item) => textFromReasoningValue(item))
      .filter(Boolean)
      .join("\n\n")
      .trim();
  }
  if (!value || typeof value !== "object") return "";
  const record = value as Record<string, unknown>;
  const parts: string[] = [];
  appendDistinctText(parts, textFromReasoningValue(record.summary));
  appendDistinctText(parts, textFromReasoningValue(record.text));
  appendDistinctText(parts, textFromReasoningValue(record.content));
  appendDistinctText(parts, textFromReasoningValue(record.delta));
  return parts.join("\n\n").trim();
}

function reasoningBlockText(block: Record<string, unknown>): string {
  const data = block.data && typeof block.data === "object"
    ? block.data as Record<string, unknown>
    : block;
  const parts: string[] = [];
  appendDistinctText(parts, textFromReasoningValue(data.summary));
  appendDistinctText(parts, textFromReasoningValue(data.text));
  appendDistinctText(parts, textFromReasoningValue(data.content));
  appendDistinctText(parts, textFromReasoningValue(block.summary));
  appendDistinctText(parts, textFromReasoningValue(block.text));
  appendDistinctText(parts, textFromReasoningValue(block.content));
  return parts.join("\n\n").trim();
}

function reasoningFrameText(frame: ConsoleFrame): string {
  const data = frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown>
    : frame.data;
  if (frame.event === "reasoning_delta" && typeof (data as Record<string, unknown>)?.delta === "string") {
    return (data as Record<string, string>).delta;
  }
  if (frame.event === "reasoning_complete") {
    // Native AgentEvent completion contains the complete block, including its
    // original whitespace. Older callers used text or a bare string.
    if (typeof data === "string") return data;
    for (const field of ["content", "text"] as const) {
      if (typeof (data as Record<string, unknown>)?.[field] === "string") {
        return (data as Record<string, string>)[field];
      }
    }
  }
  return textFromReasoningValue(data).trim();
}

const ACTIVITY_HIDDEN_EVENTS = new Set([
  ...HIDDEN_EVENTS,
  "mcp_app",
  "text_delta",
  "tool_call_requested",
  "tool_call",
  "server_tool_content",
  "tool_execution_started",
  "tool_result_received",
  "tool_execution_completed",
]);

export function mergeConversationFrames(...frameSets: Array<ConsoleFrame[] | undefined>): ConsoleFrame[] {
  const byId = new Map<string, ConsoleFrame>();
  const ordered: ConsoleFrame[] = [];

  for (const frameSet of frameSets) {
    for (const frame of frameSet || []) {
      const key = frame.id || `${frame.event}:${frame.timestampMs || 0}`;
      if (byId.has(key)) {
        continue;
      }
      byId.set(key, frame);
      ordered.push(frame);
    }
  }

  return ordered;
}

function isoFromTimestampMs(timestampMs: number | undefined): string | undefined {
  if (typeof timestampMs !== "number" || !Number.isFinite(timestampMs)) {
    return undefined;
  }
  return new Date(timestampMs).toISOString();
}

function parseToolCallId(frame: ConsoleFrame): string | null {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  if (frame.event === "server_tool_content") {
    const content = record?.content && typeof record.content === "object"
      ? record.content as Record<string, unknown>
      : null;
    const type = typeof content?.type === "string" ? content.type : "";
    const isAnnotationPayload = type === "message_annotations" || Array.isArray(content?.annotations);
    const id = isAnnotationPayload
      ? content?.item_id ?? record?.item_id ?? record?.tool_call_id
      : content?.item_id ?? content?.id ?? record?.item_id ?? record?.tool_call_id ?? record?.id;
    return typeof id === "string" && id.trim() ? id.trim() : null;
  }
  const id = record?.tool_call_id ?? record?.id;
  return typeof id === "string" && id.trim() ? id.trim() : null;
}

function parseToolName(frame: ConsoleFrame): string {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  if (frame.event === "server_tool_content") {
    const content = record?.content && typeof record.content === "object"
      ? record.content as Record<string, unknown>
      : null;
    const name = content?.name ?? record?.tool_name ?? record?.name ?? record?.kind;
    return typeof name === "string" && name.trim() ? name.trim() : "tool";
  }
  return typeof record?.name === "string" && record.name.trim() ? record.name : "tool";
}

function parseToolArguments(frame: ConsoleFrame): string {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  if (frame.event === "server_tool_content") {
    const content = record?.content && typeof record.content === "object"
      ? record.content as Record<string, unknown>
      : null;
    const action = content?.action && typeof content.action === "object"
      ? content.action as Record<string, unknown>
      : null;
    const queries = Array.isArray(action?.queries)
      ? action.queries.filter((query): query is string => typeof query === "string" && query.trim().length > 0)
      : [];
    const query = queries.length > 0
      ? queries.join("\n")
      : content?.query ?? content?.input ?? action?.query;
    return typeof query === "string" && query.trim() ? query.trim() : "";
  }
  if (typeof record?.arguments === "string" && record.arguments.trim()) {
    return record.arguments;
  }
  if ("args" in (record || {}) && record?.args !== undefined) {
    return JSON.stringify(record.args);
  }
  return JSON.stringify(record || {});
}

function normalizeToolArgumentsForSignature(argumentsText: string | undefined): string {
  const trimmed = (argumentsText || "").trim();
  if (!trimmed) return "";
  try {
    return JSON.stringify(JSON.parse(trimmed));
  } catch {
    return trimmed.replace(/\s+/g, " ");
  }
}

function toolBlockSignature(block: ConversationRichToolCallBlock): string {
  return `${block.name}\u0000${normalizeToolArgumentsForSignature(block.arguments)}`;
}

function addToolSignatureCount(
  counts: Map<string, number>,
  block: ConversationRichToolCallBlock,
): void {
  const key = toolBlockSignature(block);
  counts.set(key, (counts.get(key) || 0) + 1);
}

function consumeToolSignatureCount(
  counts: Map<string, number>,
  block: ConversationRichToolCallBlock,
): boolean {
  const key = toolBlockSignature(block);
  const count = counts.get(key) || 0;
  if (count <= 0) return false;
  if (count === 1) counts.delete(key);
  else counts.set(key, count - 1);
  return true;
}

function liveToolDedupeState(
  frames: ConsoleFrame[],
  toolBlocks: Map<string, ConversationRichToolCallBlock>,
): { liveToolCallIds: Set<string>; liveToolSignatureCounts: Map<string, number> } {
  const liveToolCallIds = new Set<string>();
  const liveToolSignatureCounts = new Map<string, number>();

  for (const frame of frames) {
    if (frame.sourceKind === "session_history") continue;
    if (
      frame.event !== "tool_call_requested"
      && frame.event !== "tool_call"
      && frame.event !== "tool_execution_started"
      && frame.event !== "server_tool_content"
    ) {
      continue;
    }
    const toolCallId = parseToolCallId(frame);
    if (!toolCallId || liveToolCallIds.has(toolCallId)) continue;
    const block = toolBlocks.get(toolCallId);
    if (!block) continue;
    liveToolCallIds.add(toolCallId);
    addToolSignatureCount(liveToolSignatureCounts, block);
  }

  return { liveToolCallIds, liveToolSignatureCounts };
}

const TECHNICAL_PEER_INTENTS = new Set(["checksum_token"]);
const PEER_PAYLOAD_TEXT_KEYS = [
  "message",
  "body",
  "text",
  "summary",
  "reply",
  "content",
  "subject",
  "question",
  "prompt",
  "description",
  "request",
  "request_subject",
  "token",
  "status_line",
];

function isTechnicalPeerIntent(intent: string | undefined): boolean {
  return Boolean(intent && TECHNICAL_PEER_INTENTS.has(intent.trim()));
}

function displayPeerIntent(intent: string | undefined): string | undefined {
  if (!intent) return undefined;
  const trimmed = intent.trim();
  if (!trimmed || isTechnicalPeerIntent(trimmed)) return undefined;
  return trimmed;
}

function parseJsonPayload(value: string): unknown {
  try {
    return JSON.parse(value);
  } catch {
    return null;
  }
}

function summarizePeerPayload(value: unknown): string | undefined {
  if (typeof value === "string") {
    const trimmed = value.trim();
    if (!trimmed) return undefined;
    if (
      (trimmed.startsWith("{") && trimmed.endsWith("}")) ||
      (trimmed.startsWith("[") && trimmed.endsWith("]")) ||
      (trimmed.startsWith("\"") && trimmed.endsWith("\""))
    ) {
      const parsed = parseJsonPayload(trimmed);
      if (parsed !== null) {
        return summarizePeerPayload(parsed) || trimmed;
      }
    }
    return trimmed.replace(/^["']|["']$/g, "");
  }
  if (Array.isArray(value)) {
    const parts = value
      .map((item) => summarizePeerPayload(item))
      .filter((item): item is string => Boolean(item));
    return parts.length ? parts.join(" ") : undefined;
  }
  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    const type = typeof record.type === "string" ? record.type : "";
    if (type === "image" || type === "image_ref" || type === "image_upload") {
      return typeof record.alt === "string" && record.alt.trim()
        ? record.alt.trim()
        : type === "image_ref"
          ? "referenced image"
          : "attached image";
    }
    for (const key of PEER_PAYLOAD_TEXT_KEYS) {
      const summary = summarizePeerPayload(record[key]);
      if (summary) return summary;
    }
    return JSON.stringify(record);
  }
  return undefined;
}

function extractLabeledCommsValue(lines: string[], label: string): string | undefined {
  const labelPattern = new RegExp(`\\b${label}:\\s*(.*)$`);
  const startIndex = lines.findIndex((line) => labelPattern.test(line));
  if (startIndex < 0) return undefined;
  const match = lines[startIndex]?.match(labelPattern);
  const first = match?.[1]?.trim() || "";
  if (!first) return undefined;

  const chunks = [first];
  const startsJson = first.startsWith("{") || first.startsWith("[");
  if (!startsJson || parseJsonPayload(first) !== null) {
    return first;
  }

  for (let i = startIndex + 1; i < lines.length; i++) {
    const next = lines[i].trim();
    if (/^(Intent|Body|Params|Request ID|Status|Result):\s*/.test(next)) break;
    chunks.push(next);
    const candidate = chunks.join("\n").trim();
    if (parseJsonPayload(candidate) !== null) return candidate;
  }

  return chunks.join("\n").trim();
}

function extractPeerBodyFromArgs(argsRecord: Record<string, unknown> | null): string | undefined {
  if (!argsRecord) return undefined;
  const directBody = summarizePeerPayload(argsRecord.body);
  if (directBody) return directBody;
  const paramsBody = summarizePeerPayload(argsRecord.params);
  if (paramsBody) return paramsBody;
  const resultBody = summarizePeerPayload(argsRecord.result);
  if (resultBody) return resultBody;
  return undefined;
}

function capturePeersResult(peerRegistry: Map<string, string>, rawResult: unknown): void {
  const resultText = typeof rawResult === "string"
    ? rawResult
    : rawResult && typeof rawResult === "object"
      ? JSON.stringify(rawResult)
      : "";
  if (!resultText) return;
  try {
    const parsed = JSON.parse(resultText) as { peers?: Array<{ peer_id?: unknown; name?: unknown }> };
    if (!Array.isArray(parsed.peers)) return;
    for (const peer of parsed.peers) {
      if (typeof peer.peer_id === "string" && typeof peer.name === "string") {
        peerRegistry.set(peer.peer_id, peer.name);
      }
    }
  } catch {
    // ignore non-JSON peers payloads
  }
}

function peerTargetFromArgs(
  argsRecord: Record<string, unknown> | null,
  peerRegistry?: Map<string, string>,
): string | undefined {
  const peerId = typeof argsRecord?.peer_id === "string" ? argsRecord.peer_id.trim() : "";
  const registryName = peerId ? peerRegistry?.get(peerId) : undefined;
  return registryName
    ? peerLastSegment(registryName)
    : typeof argsRecord?.display_name === "string" && argsRecord.display_name.trim()
      ? peerLastSegment(argsRecord.display_name.trim())
      : typeof argsRecord?.to === "string" && argsRecord.to.trim()
        ? peerLastSegment(argsRecord.to.trim())
        : peerId
          ? peerId.slice(0, 8)
          : undefined;
}

type ParsedToolResult = {
  result?: string;
  images?: ConversationRichImageBlock[];
  status: "pending" | "success" | "error";
  completionEvidence: ToolCompletionEvidence;
};

function parseToolResult(frame: ConsoleFrame, blobBaseUrl?: string): ParsedToolResult {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  const completionEvidence = toolCompletionFromFrame(frame, parseToolCallId(frame) || "");
  const status = completionEvidence.outcome === "success" ? "success" : completionEvidence.outcome === "error" ? "error" : "pending";
  const feedback = operationFeedbackFromFrame(frame);
  const { text, images } = toolResultTextAndImages(record, blobBaseUrl);
  const result = feedback ? feedback.detail : text;
  return {
    ...(result !== undefined ? { result } : {}),
    ...(images.length > 0 ? { images } : {}),
    status,
    completionEvidence,
  };
}

/** What a later result frame for a call adds to what is known of it. A
 * tool's output travels on its result event; the execution fact for the
 * same call carries no body and is no completion evidence. So a frame
 * without a body never clears an earlier one, and a frame without evidence
 * never unsettles a settled call, whichever of the two comes first. */
function laterToolResult(
  current: { completionEvidence?: ToolCompletionEvidence },
  next: ParsedToolResult,
): Partial<ParsedToolResult> {
  const keepsOutcome = next.completionEvidence.outcome === "unknown"
    && settledToolCompletion(current.completionEvidence);
  return {
    ...(next.result !== undefined ? { result: next.result } : {}),
    ...(next.images ? { images: next.images } : {}),
    ...(keepsOutcome ? {} : { status: next.status, completionEvidence: next.completionEvidence }),
  };
}

function buildToolBlocks(frames: ConsoleFrame[], blobBaseUrl?: string): Map<string, ConversationRichToolCallBlock> {
  const toolCalls = new Map<string, ConversationRichToolCallBlock>();
  const pendingResults = new Map<string, ParsedToolResult>();
  // Peer registry built from `peers` tool results: peer_id (uuid) -> name.
  // The LLM-supplied `display_name` field on send_* args is unreliable
  // (agents have been observed filling it with their own name on every
  // call), so we derive the recipient label from the canonical `peers`
  // listing whenever possible. The list is ordered, so by the time a
  // send_request appears the relevant peers result has already been
  // processed.
  const peerRegistry = buildPeerRegistry(frames);

  for (const frame of frames) {
    if (frame.event === "server_tool_content") {
      const toolCallId = parseToolCallId(frame);
      const parsed = serverToolContentSummary(frame);
      if (!toolCallId) {
        if (parsed && parsed.status !== "pending") {
          const name = parseToolName(frame);
          const targetName = name.replace(/_annotations$/, "");
          const existing = [...toolCalls.values()].reverse().find((block) =>
            !block.result &&
            (block.name === targetName || block.name === name || name.startsWith(block.name))
          );
          if (existing) {
            toolCalls.set(existing.toolCallId, {
              ...existing,
              ...(parsed.result !== undefined ? { result: parsed.result } : {}),
              status: parsed.status,
            });
          }
        }
        continue;
      }
      const existing = toolCalls.get(toolCallId);
      if (existing) {
        toolCalls.set(toolCallId, {
          ...existing,
          ...(parsed?.result ? { result: parsed.result } : {}),
          status: parsed?.status || existing.status,
        });
        continue;
      }
      toolCalls.set(toolCallId, {
        type: "tool-call",
        toolCallId,
        name: parseToolName(frame),
        arguments: parseToolArguments(frame),
        ...(parsed?.result ? { result: parsed.result } : {}),
        status: parsed?.status || "pending",
      });
      continue;
    }

    if (frame.event === "tool_result_received" || frame.event === "tool_execution_completed" || frame.event === "tool_execution_timed_out") {
      const toolCallId = parseToolCallId(frame);
      const data = frame.data as Record<string, unknown> | undefined;
      // Capture peer registry from the `peers` tool result.
      if (data && (data.name === "peers" || data.tool_name === "peers")) {
        capturePeersResult(peerRegistry, data.result);
      }
      if (!toolCallId) continue;
      const parsed = parseToolResult(frame, blobBaseUrl);
      const current = toolCalls.get(toolCallId);
      if (current) {
        toolCalls.set(toolCallId, { ...current, ...laterToolResult(current, parsed) });
      } else {
        const pending = pendingResults.get(toolCallId);
        pendingResults.set(toolCallId, pending ? { ...pending, ...laterToolResult(pending, parsed) } : parsed);
      }
    }

    if (frame.event === "tool_call_requested" || frame.event === "tool_call" || frame.event === "tool_execution_started") {
      const toolCallId = parseToolCallId(frame);
      if (!toolCallId || toolCalls.has(toolCallId)) continue;
      const pending = pendingResults.get(toolCallId);
      const name = parseToolName(frame);
      const args = frame.data && typeof frame.data === "object" ? (frame.data as Record<string, unknown>).args : null;
      const argsRecord = args && typeof args === "object" ? args as Record<string, unknown> : null;

      // Extract peer comms metadata for send_* tools.
      //
      // Wire shape: send_* tools take `peer_id` (UUID, authoritative) +
      // an optional `display_name` hint. The hint is LLM-controlled and
      // has been observed to be wrong (agents fill it with their own
      // name), so we resolve `peer_id` against the peer registry built
      // from the most recent `peers` tool result. Fall back order:
      // registry → display_name → short peer_id → legacy `to` field.
      const isPeerTool = name === "send_request" || name === "send_message" || name === "send_response";
      const peerTarget = isPeerTool ? peerTargetFromArgs(argsRecord, peerRegistry) : undefined;
      const rawPeerIntent = isPeerTool && typeof argsRecord?.intent === "string"
        ? argsRecord.intent as string
        : undefined;
      const peerIntent = displayPeerIntent(rawPeerIntent);
      const peerBody = isPeerTool ? extractPeerBodyFromArgs(argsRecord) : undefined;

      toolCalls.set(toolCallId, {
        type: "tool-call",
        toolCallId,
        name,
        arguments: parseToolArguments(frame),
        ...(pending?.result !== undefined ? { result: pending.result } : {}),
        ...(pending?.images ? { images: pending.images } : {}),
        status: pending?.status || "pending",
        completionEvidence: pending?.completionEvidence ?? { outcome: "running", source: "runtime-start", toolCallId },
        ...(peerTarget ? { peerTarget } : {}),
        ...(isPeerTool ? { peerIdentity: typeof argsRecord?.peer_id === "string" ? argsRecord.peer_id : typeof argsRecord?.to === "string" ? argsRecord.to : "Unknown peer" } : {}),
        ...(isPeerTool && typeof argsRecord?.peer_id === "string" && peerRegistry?.get(argsRecord.peer_id)?.trim()
          ? { peerDisplayLabel: decodeMemberAlias(peerLastSegment(peerRegistry.get(argsRecord.peer_id)!.trim())) } : {}),
        ...(peerIntent ? { peerIntent } : {}),
        ...(peerBody ? { peerBody, peerBodyFormat: "verbatim" as const } : {}),
      });
    }
  }

  return toolCalls;
}

function buildPeerRegistry(frames: ConsoleFrame[]): Map<string, string> {
  const peerRegistry = new Map<string, string>();
  for (const frame of frames) {
    if (frame.event !== "tool_result_received" && frame.event !== "tool_execution_completed") continue;
    const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
    if (!data || (data.name !== "peers" && data.tool_name !== "peers")) continue;
    capturePeersResult(peerRegistry, data.result);
  }
  return peerRegistry;
}

function parsePeerSummary(text: string): { verb: string; summary: string } | null {
  // Match "Peer response: ..." / "Peer request: ..." / "Peer message: ..." at any position
  const match = text.match(/Peer\s+(response|request|message):\s*(.+?)(?:\s*Status:\s|$)/s);
  if (!match) return null;

  const [, verb, body] = match;
  let summary = body.trim();

  // Try to parse JSON payload and extract clean text
  try {
    const parsed = JSON.parse(summary);
    if (typeof parsed === "object" && parsed !== null) {
      if (typeof parsed.summary === "string") summary = parsed.summary;
      else if (typeof parsed.text === "string") summary = parsed.text;
      else if (typeof parsed.body === "string") summary = parsed.body;
      else if (typeof parsed.message === "string") summary = parsed.message;
    }
  } catch {
    summary = summary.replace(/^["']|["']$/g, "");
  }

  return { verb, summary };
}

function renderPeerEntry(
  frame: ConsoleFrame,
  entryId: string,
): ConversationTimelineEntry | null {
  const rawText = summarizeFrameData(frame.data);
  if (!rawText) return null;

  const peer = parsePeerSummary(rawText);
  if (!peer) return null;

  return {
    kind: "message",
    id: entryId,
    identity: SYSTEM_IDENTITY,
    variant: "meta",
    createdAt: isoFromTimestampMs(frame.timestampMs),
    text: `↩ ${peer.verb}: ${peer.summary}`,
  };
}

function renderTerminalEntry(
  agent: ConsoleAgent | null,
  frame: ConsoleFrame,
  entryId: string,
  streamedText = "",
  textMode: ConversationTextMode = "legacy",
): ConversationTimelineEntry | null {
  if (frame.event === "interaction_complete" || frame.event === "run_completed" || frame.event === "text_complete") {
    if (isSteerDeliveryTerminalFrame(frame)) return null;
    const source = terminalFrameVisibleText(frame);
    const text = textMode === "markdown" ? source : source.trim();
    if (!text) return null;

    // Peer responses always render as compact meta, even if text was streamed
    const peer = parsePeerSummary(text);
    if (peer) {
      return {
        kind: "message",
        id: entryId,
        identity: SYSTEM_IDENTITY,
        variant: "meta",
        createdAt: isoFromTimestampMs(frame.timestampMs),
        text: `↩ ${peer.verb}: ${peer.summary}`,
      };
    }

    if (streamedTextMatchesTerminal(streamedText, text)) {
      return null;
    }

    const blocks = messageTextBlocks(text, textMode);
    return {
      kind: "message",
      id: entryId,
      identity: agentIdentity(agent),
      variant: blocks.length > 0 ? "rich" : "plain",
      createdAt: isoFromTimestampMs(frame.timestampMs),
      ...(blocks.length > 0 ? { blocks } : { text }),
    };
  }

  if (frame.event === "interaction_failed" || frame.event === "run_failed") {
    const runtimeEvent = runtimeEventFromFrame(frame.event, frame.data);
    return {
      kind: "message",
      id: entryId,
      identity: SYSTEM_IDENTITY,
      variant: "meta",
      createdAt: isoFromTimestampMs(frame.timestampMs),
      text: runtimeEventText(runtimeEvent),
      runtimeEvent,
    };
  }

  return null;
}

function terminalFrameVisibleText(frame: ConsoleFrame): string {
  if (isSteerDeliveryTerminalFrame(frame)) return "";
  if (frame.event === "text_complete") {
    const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
    if (typeof record?.content === "string") return record.content;
    if (typeof record?.text === "string") return record.text;
  }
  if (
    frame.event === "interaction_complete"
    || frame.event === "run_completed"
    || frame.event === "text_complete"
  ) {
    return summarizeFrameData(frame.data);
  }
  return "";
}

type AssistantFrameOwner = Pick<ConsoleFrame, "runId" | "interactionId" | "runtimeKey" | "identity" | "sessionId" | "data" | "id" | "event">;

function ownerContextsConflict(left: Pick<ConsoleFrame, "runtimeKey" | "identity" | "sessionId">, right: Pick<ConsoleFrame, "runtimeKey" | "identity" | "sessionId">): boolean {
  return (["runtimeKey", "identity", "sessionId"] as const).some((key) => (
    Boolean(left[key] && right[key] && left[key] !== right[key])
  ));
}

function sameAssistantRunOwner(left: AssistantFrameOwner, right: AssistantFrameOwner): boolean {
  if (ownerContextsConflict(left, right)) return false;
  const leftRun = left.runId?.trim() || "";
  const rightRun = right.runId?.trim() || "";
  const leftInteraction = left.interactionId?.trim() || "";
  const rightInteraction = right.interactionId?.trim() || "";
  if (leftRun || rightRun) {
    return Boolean(leftRun && leftRun === rightRun
      && !(leftInteraction && rightInteraction && leftInteraction !== rightInteraction));
  }
  return UUID_FORM.test(leftInteraction) && UUID_FORM.test(rightInteraction)
    && leftInteraction.toLowerCase() === rightInteraction.toLowerCase();
}

function sameTextStreamOwner(left: AssistantFrameOwner | undefined, right: AssistantFrameOwner): boolean {
  if (left && (isRealtimeHistoryMessage(left) || isRealtimeHistoryMessage(right))) {
    return left === right;
  }
  if (left && (hasAssistantMessageIdCarrier(left) || hasAssistantMessageIdCarrier(right))) {
    const key = assistantMessageKey(left);
    return Boolean(key && key === assistantMessageKey(right) && !ownerContextsConflict(left, right));
  }
  return Boolean(left && !ownerContextsConflict(left, right)
    && (left.runId?.trim() || "") === (right.runId?.trim() || "")
    && (left.interactionId?.trim() || "") === (right.interactionId?.trim() || ""));
}

type LiveAssistantTextOccurrence = {
  owner: ConsoleFrame;
  frames: ConsoleFrame[];
  chunks: string[];
  text: string;
  complete: boolean;
};

function assistantOwnerKey(frame: AssistantFrameOwner): string {
  const messageKey = assistantMessageKey(frame);
  if (messageKey) return `assistant:${messageKey}`;
  if (hasAssistantMessageIdCarrier(frame)) return `unbound:${frame.id}`;
  if (frame.runId?.trim()) return `run:${frame.runId.trim()}`;
  const interaction = frame.interactionId?.trim() || "";
  return UUID_FORM.test(interaction) ? `interaction:${interaction.toLowerCase()}` : "legacy";
}

function canonicalHistoryAssistantText(frame: ConsoleFrame): string | undefined {
  if (frame.sourceKind !== "session_history") return undefined;
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
  const message = record.message && typeof record.message === "object" ? record.message as Record<string, unknown> : {};
  if (message.role !== "assistant" && message.role !== "block_assistant") return undefined;
  // The backend owns text_blocks().join(""). Reconstructing it with paragraph
  // separators or dropping whitespace-only blocks changes the source bytes.
  if (typeof record.text === "string") return record.text;
  if (typeof record.result === "string") return record.result;
  return undefined;
}

function historyAssistantSource(frame: ConsoleFrame): string {
  const canonical = canonicalHistoryAssistantText(frame);
  if (canonical !== undefined) return canonical;
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
  const parsed = historyMessageText(record.message);
  return parsed.role === "assistant" ? parsed.text : terminalFrameVisibleText(frame);
}

function buildAssistantHistoryReconciliation(frames: ConsoleFrame[], renderTextDeltas: boolean): {
  consumedHistory: Set<string>;
  deltaOverrides: Map<string, string>;
  consumeReasoning: (history: ConsoleFrame, text: string) => boolean;
} {
  const byOwner = new Map<string, LiveAssistantTextOccurrence[]>();
  const active = new Map<string, LiveAssistantTextOccurrence>();
  const reasoningByOwner = new Map<string, Map<string, ConsoleFrame[]>>();
  const consumedReasoning = new Set<string>();
  const pushOccurrence = (frame: ConsoleFrame): LiveAssistantTextOccurrence => {
    const occurrence = { owner: frame, frames: [], chunks: [], text: "", complete: false };
    const key = assistantOwnerKey(frame);
    const bucket = byOwner.get(key) || [];
    bucket.push(occurrence);
    byOwner.set(key, bucket);
    active.set(key, occurrence);
    return occurrence;
  };
  for (const frame of frames) {
    if (frame.sourceKind === "session_history" || hasAssistantMessageIdCarrier(frame)) continue;
    const key = assistantOwnerKey(frame);
    if (frame.event === "reasoning_complete" && frame.runId?.trim()) {
      const byText = reasoningByOwner.get(key) || new Map<string, ConsoleFrame[]>();
      const text = reasoningFrameText(frame);
      const completions = byText.get(text) || [];
      completions.push(frame);
      byText.set(text, completions);
      reasoningByOwner.set(key, byText);
    }
    let current = active.get(key);
    if (current && !sameTextStreamOwner(current.owner, frame)) current = undefined;
    if (frame.event === "text_delta" && renderTextDeltas) {
      if (!current || current.complete) current = pushOccurrence(frame);
      current.frames.push(frame);
      current.chunks.push(summarizeFrameData(frame.data));
      continue;
    }
    const terminal = terminalFrameVisibleText(frame);
    if (terminal) {
      const streamed = current?.chunks.join("") || "";
      if (current && (current.text || streamed) === terminal) {
        current.text = terminal;
        current.complete = true;
      } else {
        current = pushOccurrence(frame);
        current.text = terminal;
        current.complete = true;
      }
      continue;
    }
    // A visible live boundary ends this text occurrence. History projections
    // never change a live stream's message boundaries.
    if (current && (frame.event.startsWith("tool_") || frame.event.startsWith("reasoning_")
      || frame.event === "assistant_image" || frame.event === "assistant_image_appended"
      || frame.event === "user_input" || frame.event === "system_notice")) {
      current.complete = true;
      active.delete(key);
    }
  }
  for (const bucket of byOwner.values()) {
    for (const occurrence of bucket) occurrence.text ||= occurrence.chunks.join("");
  }
  const consumed = new Set<LiveAssistantTextOccurrence>();
  const consumedHistory = new Set<string>();
  const deltaOverrides = new Map<string, string>();
  for (const history of frames) {
    if (history.sourceKind !== "session_history" || hasAssistantMessageIdCarrier(history)
      || isRealtimeHistoryMessage(history)) continue;
    const text = historyAssistantSource(history);
    if (!text) continue;
    const key = assistantOwnerKey(history);
    const candidates = byOwner.get(key) || [];
    const occurrence = candidates.find((candidate) => {
      if (consumed.has(candidate) || !candidate.text) return false;
      if (key === "legacy") return !candidate.owner.runId?.trim() && candidate.text === text;
      if (!sameAssistantRunOwner(history, candidate.owner)) return false;
      // Legacy interaction-only history has no message identity; retain the
      // existing compatibility join. Typed runs use exact authored source.
      if (!history.runId?.trim()) return true;
      return candidate.text === text
        || (!candidate.complete && candidate.frames.length > 0 && text.startsWith(candidate.text));
    });
    if (!occurrence) continue;
    consumed.add(occurrence);
    consumedHistory.add(history.id);
    if (occurrence.text !== text && !occurrence.complete && occurrence.frames.length > 0) {
      // Durable text can beat the final live chunks. Complete the last
      // observed chunk, preserving earlier segments and their chronological
      // position around an overlapping input or another run's output.
      const last = occurrence.frames.length - 1;
      deltaOverrides.set(occurrence.frames[last].id, occurrence.chunks[last] + text.slice(occurrence.text.length));
    }
  }
  return {
    consumedHistory,
    deltaOverrides,
    consumeReasoning: (history, text) => {
      if (hasAssistantMessageIdCarrier(history) || isRealtimeHistoryMessage(history)) return false;
      const candidates = reasoningByOwner.get(assistantOwnerKey(history))?.get(text) || [];
      const completion = candidates.find((live) => !consumedReasoning.has(live.id)
        && sameAssistantRunOwner(history, live));
      if (!completion) return false;
      consumedReasoning.add(completion.id);
      return true;
    },
  };
}

function historyHasAssistantSiblings(frame: ConsoleFrame): boolean {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
  const message = record.message && typeof record.message === "object" ? record.message as Record<string, unknown> : {};
  return message.role === "block_assistant" && Array.isArray(message.blocks)
    && message.blocks.some((block) => {
      if (!block || typeof block !== "object") return false;
      const item = block as Record<string, unknown>;
      return (item.block_type || item.type) !== "text";
    });
}

function buildBlobUrl(blobId: string, baseUrl?: string): string {
  const path = `/blobs/${encodeURIComponent(blobId)}`;
  const base = baseUrl?.trim();
  if (!base) return path;
  return `${base.replace(/\/+$/, "")}${path}`;
}

function renderAssistantImageEntry(
  agent: ConsoleAgent | null,
  frame: ConsoleFrame,
  entryId: string,
  blobBaseUrl?: string,
): ConversationTimelineEntry | null {
  const data = frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown>
    : {};
  const image = data.image && typeof data.image === "object"
    ? data.image as Record<string, unknown>
    : data;
  const blobRef = image.blob_ref && typeof image.blob_ref === "object"
    ? image.blob_ref as Record<string, unknown>
    : null;
  const blobId = typeof image.blob_id === "string"
    ? image.blob_id
    : typeof blobRef?.blob_id === "string"
      ? blobRef.blob_id
      : "";
  if (!blobId) return null;
  const mediaType = typeof image.media_type === "string"
    ? image.media_type
    : typeof blobRef?.media_type === "string"
      ? blobRef.media_type
      : "image/png";
  const width = typeof image.width === "number" ? image.width : undefined;
  const height = typeof image.height === "number" ? image.height : undefined;
  const imageId = typeof image.image_id === "string" ? image.image_id : undefined;
  return {
    kind: "message",
    id: entryId,
    identity: agentIdentity(agent),
    variant: "rich",
    createdAt: isoFromTimestampMs(frame.timestampMs),
    blocks: [{
      type: "image",
      src: buildBlobUrl(blobId, blobBaseUrl),
      mediaType,
      alt: "generated image",
      ...(width !== undefined ? { width } : {}),
      ...(height !== undefined ? { height } : {}),
      blobId,
      ...(imageId ? { imageId } : {}),
    }],
  };
}

function renderGeneratedImageToolResultEntries(
  agent: ConsoleAgent | null,
  frame: ConsoleFrame,
  entryId: string,
  blobBaseUrl?: string,
): ConversationTimelineEntry[] {
  const data = frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown>
    : {};
  const name = typeof data.name === "string"
    ? data.name
    : typeof data.tool_name === "string"
      ? data.tool_name
      : "";
  if (name !== "generate_image") return [];

  let result = data.result;
  if (typeof result === "string") {
    try {
      result = JSON.parse(result);
    } catch {
      return [];
    }
  }
  if (!result || typeof result !== "object") return [];
  const images = (result as Record<string, unknown>).images;
  if (!Array.isArray(images)) return [];

  return images.flatMap((image, index) => {
    if (!image || typeof image !== "object") return [];
    const imageFrame: ConsoleFrame = {
      ...frame,
      data: { image },
    };
    const imageEntry = renderAssistantImageEntry(
      agent,
      imageFrame,
      `${entryId}:generated-image:${index}`,
      blobBaseUrl,
    );
    return imageEntry ? [imageEntry] : [];
  });
}

function imageEntryKey(entry: ConversationTimelineEntry, preferIdentity = false): string | null {
  if (entry.kind !== "message" || entry.variant !== "rich" || !("blocks" in entry)) {
    return null;
  }
  const block = entry.blocks?.[0];
  if (!block || block.type !== "image") return null;
  if (preferIdentity && typeof block.imageId === "string" && block.imageId.trim()) {
    return `image:${block.imageId}`;
  }
  if (typeof block.blobId === "string" && block.blobId.trim()) {
    return `blob:${block.blobId.trim()}`;
  }
  if (typeof block.imageId === "string" && block.imageId.trim()) {
    return `image:${block.imageId.trim()}`;
  }
  if (typeof block.src === "string" && block.src.trim()) {
    return `src:${block.src.trim()}`;
  }
  return null;
}

function normalizeComparableText(value: string): string {
  return value.replace(/\s+/g, " ").trim();
}

function normalizeTextWithoutWhitespace(value: string): string {
  return value.replace(/\s+/g, "");
}

function streamedTextMatchesTerminal(streamedText: string, terminalText: string): boolean {
  const streamed = normalizeComparableText(streamedText);
  const terminal = normalizeComparableText(terminalText);
  if (!streamed || !terminal) return false;
  return streamed === terminal
    || normalizeTextWithoutWhitespace(streamed) === normalizeTextWithoutWhitespace(terminal);
}

function conversationEntryVisibleText(entry: ConversationTimelineEntry): string {
  if (entry.kind !== "message") return "";
  if ("text" in entry && typeof entry.text === "string") return entry.text;
  if (!("blocks" in entry) || !Array.isArray(entry.blocks)) return "";
  return entry.blocks
    .map((block) => {
      if (!block || typeof block !== "object") return "";
      const record = block as unknown as Record<string, unknown>;
      if (record.type === "thinking") return "";
      if (record.type === "markdown" && typeof record.source === "string") return record.source;
      if (typeof record.text === "string") return record.text;
      if (typeof record.peerBody === "string") return record.peerBody;
      return "";
    })
    .filter(Boolean)
    .join("\n");
}

const UUID_FORM = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

function shouldSuppressRepeatedAssistantEntry(
  entry: ConversationTimelineEntry,
  priorEntries: ConversationTimelineEntry[],
  protectedEntryIds: ReadonlySet<string>,
): boolean {
  if (entry.kind !== "message" || protectedEntryIds.has(entry.id)) return false;
  if (entry.identity.id === USER_IDENTITY.id || entry.identity.id === COMMS_IDENTITY.id || entry.identity.id === SYSTEM_IDENTITY.id) {
    return false;
  }
  const signature = normalizeComparableText(conversationEntryVisibleText(entry));
  if (!signature) return false;
  const entryRun = entry.runId?.trim() || "";
  const entryInteraction = entry.interactionId?.trim() || "";
  const entryTs = Date.parse(String(entry.createdAt || ""));
  for (let index = priorEntries.length - 1; index >= 0; index--) {
    const prior = priorEntries[index];
    if (prior.kind !== "message" || protectedEntryIds.has(prior.id)) continue;
    if (prior.identity.id === USER_IDENTITY.id) {
      const userText = normalizeComparableText(conversationEntryVisibleText(prior));
      if (userText) return false;
      continue;
    }
    if (prior.identity.id !== entry.identity.id) continue;
    const priorSignature = normalizeComparableText(conversationEntryVisibleText(prior));
    if (priorSignature !== signature) continue;
    const priorRun = prior.runId?.trim() || "";
    const priorInteraction = prior.interactionId?.trim() || "";
    if (entryRun || priorRun) {
      // A run owns many committed messages. Only an actual source-frame
      // replay can be discarded here; live/history occurrences join earlier.
      if (entryRun === priorRun && entry.id === prior.id) return true;
      continue;
    }
    if (UUID_FORM.test(entryInteraction) && UUID_FORM.test(priorInteraction)) {
      if (entryInteraction.toLowerCase() === priorInteraction.toLowerCase()) return true;
      continue;
    }
    const priorTs = Date.parse(String(prior.createdAt || ""));
    if (Number.isFinite(entryTs) && Number.isFinite(priorTs) && Math.abs(entryTs - priorTs) > 15_000) {
      return false;
    }
    return true;
  }
  return false;
}

export function buildQuickPromptSuggestions(agent: ConsoleAgent | null): ConversationEmptySuggestion[] {
  const labels = agent?.labels ?? {};
  const suggestions: ConversationEmptySuggestion[] = [];
  for (let index = 1; index <= 4; index++) {
    const label = labels[`console_prompt_${index}_label`]?.trim();
    const value = labels[`console_prompt_${index}_value`]?.trim();
    if (!label || !value) continue;
    suggestions.push({
      id: `prompt-${index}`,
      label,
      value,
      iconName: "i-bolt",
    });
  }
  return suggestions;
}

function renderHistoryUserEntry(
  frame: ConsoleFrame,
  entryId: string,
  blobBaseUrl?: string,
  textMode: ConversationTextMode = "legacy",
): ConversationTimelineEntry | null {
  if (
    frame.event !== "interaction_started"
    && frame.event !== "user_input"
  ) {
    return null;
  }
  if (typeof frame.data !== "object" || frame.data === null) {
    return null;
  }
  const record = frame.data as Record<string, unknown>;
  const content = record.content;
  // Typed provenance only (send origin, persisted render class); the text
  // itself never decides what kind of message this is.
  const origin = entryOriginFromFrameData(record);
  const realtimeOrigin = realtimeMessageOrigin(frame);
  if (Array.isArray(content)) {
    const contextMessage = parseConsoleContextMessage(content);
    const blocks = contentToUserBlocks(content, blobBaseUrl, textMode);
    if (blocks.length === 0) return null;
    return {
      kind: "message",
      id: entryId,
      identity: USER_IDENTITY,
      ...(realtimeOrigin ? { realtimeOrigin } : {}),
      ...(frame.interactionId ? { interactionId: frame.interactionId } : {}),
      variant: "rich",
      createdAt: isoFromTimestampMs(frame.timestampMs),
      blocks,
      ...(contextMessage ? { contextMessage, copyText: content.map((block) => block.text).join("\n\n") } : {}),
      ...(origin ? { origin } : {}),
    };
  }
  const source = extractTextFromContentBlocks(content);
  const text = textMode === "markdown" ? source : source.trim();
  if (!text) return null;
  return {
    kind: "message",
    id: entryId,
    identity: USER_IDENTITY,
    ...(realtimeOrigin ? { realtimeOrigin } : {}),
    ...(frame.interactionId ? { interactionId: frame.interactionId } : {}),
    variant: textMode === "markdown" ? "rich" : "plain",
    ...(textMode === "markdown" ? { blocks: messageTextBlocks(text, textMode) } : {}),
    createdAt: isoFromTimestampMs(frame.timestampMs),
    text,
    ...(origin ? { origin } : {}),
  };
}

function renderUserDeliveryFailureEntry(frame: ConsoleFrame, entryId: string): ConversationTimelineEntry | null {
  if (frame.event !== "user_input" || frame.status !== "delivery_failed") return null;
  return {
    kind: "message",
    id: `${entryId}:delivery-status`,
    identity: SYSTEM_IDENTITY,
    variant: "meta",
    createdAt: isoFromTimestampMs(frame.timestampMs),
    ...(frame.interactionId ? { interactionId: frame.interactionId } : {}),
    text: "Message delivery failed.",
    runtimeEvent: runtimeEventFromFrame(frame.event, {
      status: frame.status,
      input_frame_id: frame.id,
      payload: frame.data,
    }),
  };
}

function userEntryTextSignature(entry: ConversationTimelineEntry): string {
  if (entry.kind !== "message") return "";
  if ("text" in entry && typeof entry.text === "string") {
    return entry.text.replace(/\s+/g, " ").trim();
  }
  if ("blocks" in entry && Array.isArray(entry.blocks)) {
    return JSON.stringify(entry.blocks);
  }
  return "";
}

function userEntryDedupeKey(frame: ConsoleFrame, entry: ConversationTimelineEntry): string {
  if (isRealtimeHistoryMessage(frame)) {
    const origin = realtimeMessageOrigin(frame);
    return origin
      ? `realtime-user:${JSON.stringify([frame.runtimeKey, frame.identity, origin.sessionId, origin.channelId, origin.canonicalRowSequence])}`
      : `unbound-realtime-user:${frame.id}`;
  }
  const renderKey = userMessageRenderKey(frame);
  if (renderKey) return renderKey;
  const interactionId = frame.interactionId?.trim();
  if (interactionId) return `interaction:${interactionId}`;
  const signature = userEntryTextSignature(entry);
  if (frame.sourceKind === "session_history" && /^You are\b/i.test(signature)) {
    return `history-kickoff:${signature}`;
  }
  const occurrence =
    typeof frame.timestampMs === "number"
      ? `ts:${frame.timestampMs}`
      : frame.cursor
        ? `cursor:${frame.cursor}`
        : `frame:${frame.id}`;
  return signature ? `content:${occurrence}:${signature}` : "";
}

function userPromptDedupeKey(frame: ConsoleFrame, entry: ConversationTimelineEntry): string {
  return userEntryDedupeKey(frame, entry);
}

function renderRunStartedPromptEntries(
  frame: ConsoleFrame,
  entryId: string,
  options: {
    suppressEmbeddedRpcPrompt?: boolean;
    suppressStructuredCommsPrompt?: boolean;
    blobBaseUrl?: string;
    textMode?: ConversationTextMode;
  } = {},
): ConversationTimelineEntry[] {
  if (frame.event !== "run_started" || typeof frame.data !== "object" || frame.data === null) {
    return [];
  }
  const record = frame.data as Record<string, unknown>;
  const textMode = options.textMode ?? "legacy";
  const promptBlocks = contentToUserBlocks(record.prompt, options.blobBaseUrl, textMode);
  const source = extractPromptText(record.prompt);
  const prompt = textMode === "markdown" ? source : source.trim();
  if (!prompt) {
    return [];
  }
  if (isCommsLikeRunStartedPrompt(prompt) && runStartedPromptHasImagePlaceholder(frame)) {
    return [];
  }
  if (options.suppressStructuredCommsPrompt) {
    return [];
  }
  const createdAt = isoFromTimestampMs(frame.timestampMs);
  const entries: ConversationTimelineEntry[] = [];

  if (!options.suppressEmbeddedRpcPrompt) {
    if (promptBlocks.length > 0 && promptBlocks.some((block) => block.type === "image")) {
      entries.push({
        kind: "message",
        id: entryId,
        identity: USER_IDENTITY,
        ...(frame.interactionId ? { interactionId: frame.interactionId } : {}),
        variant: "rich",
        ...(createdAt ? { createdAt } : {}),
        blocks: promptBlocks,
      });
      return entries;
    }
    entries.push({
      kind: "message",
      id: entryId,
      identity: USER_IDENTITY,
      ...(frame.interactionId ? { interactionId: frame.interactionId } : {}),
      variant: textMode === "markdown" ? "rich" : "plain",
      ...(textMode === "markdown" ? { blocks: promptBlocks } : {}),
      ...(createdAt ? { createdAt } : {}),
      text: prompt,
    });
  }

  return entries;
}

function extractTextFromContentBlocks(blocks: unknown): string {
  if (typeof blocks === "string") {
    return blocks;
  }
  if (!Array.isArray(blocks)) {
    return "";
  }
  return blocks
    .map((block) => {
      if (typeof block === "string") return block;
      if (!block || typeof block !== "object") return "";
      const record = block as Record<string, unknown>;
      if (typeof record.text === "string") return record.text;
      if (typeof record.content === "string") return record.content;
      return "";
    })
    .filter((value) => value.trim().length > 0)
    .join("");
}

function extractPromptText(prompt: unknown): string {
  if (typeof prompt === "string") return prompt;
  if (!Array.isArray(prompt)) return "";
  return prompt
    .map((block) => {
      if (typeof block === "string") return block;
      if (!block || typeof block !== "object") return "";
      const record = block as Record<string, unknown>;
      if (typeof record.text === "string") return record.text;
      if (typeof record.content === "string") return record.content;
      return "";
    })
    .filter((value) => value.trim().length > 0)
    .join("\n");
}

function contentToUserBlocks(content: unknown, blobBaseUrl?: string, textMode: ConversationTextMode = "legacy"): ConversationRichBlock[] {
  if (typeof content === "string") {
    return messageTextBlocks(content, textMode);
  }
  if (!Array.isArray(content)) {
    return [];
  }
  const blocks: ConversationRichBlock[] = [];
  for (const block of content) {
    if (typeof block === "string") {
      blocks.push(...messageTextBlocks(block, textMode));
      continue;
    }
    if (!block || typeof block !== "object") continue;
    const record = block as Record<string, unknown>;
    const type = typeof record.type === "string" ? record.type : "";
    if (type === "text") {
      const text = typeof record.text === "string"
        ? record.text
        : typeof record.content === "string"
          ? record.content
          : "";
      blocks.push(...messageTextBlocks(text, textMode));
      continue;
    }
    if (type === "image" || type === "image_ref") {
      const image = record.image && typeof record.image === "object"
        ? record.image as Record<string, unknown>
        : record;
      const blobRef = image.blob_ref && typeof image.blob_ref === "object"
        ? image.blob_ref as Record<string, unknown>
        : image.blobRef && typeof image.blobRef === "object"
          ? image.blobRef as Record<string, unknown>
          : null;
      const source = typeof image.source === "string" ? image.source : "";
      const blobId = typeof record.blob_id === "string"
        ? record.blob_id
        : typeof image.blob_id === "string"
          ? image.blob_id
          : typeof record.blobId === "string"
            ? record.blobId
            : typeof image.blobId === "string"
              ? image.blobId
              : typeof blobRef?.blob_id === "string"
                ? blobRef.blob_id
                : typeof blobRef?.blobId === "string"
                  ? blobRef.blobId
                  : "";
      const mediaType = typeof image.media_type === "string"
        ? image.media_type
        : typeof image.mediaType === "string"
          ? image.mediaType
          : typeof blobRef?.media_type === "string"
            ? blobRef.media_type
            : typeof blobRef?.mediaType === "string"
              ? blobRef.mediaType
              : "image/png";
      const inlineData = typeof image.data === "string"
        ? image.data
        : typeof image.base64 === "string"
          ? image.base64
          : "";
      const directSrc = typeof image.src === "string" && image.src.trim()
        ? image.src.trim()
        : typeof image.url === "string" && image.url.trim()
          ? image.url.trim()
          : "";
      const src = blobId && (source === "blob" || !directSrc)
        ? buildBlobUrl(blobId, blobBaseUrl)
        : inlineData
          ? `data:${mediaType};base64,${inlineData}`
          : directSrc;
      if (!src) continue;
      const alt = typeof image.alt === "string" && image.alt.trim()
        ? image.alt.trim()
        : type === "image_ref"
          ? "referenced image"
          : "attached image";
      const width = typeof image.width === "number" ? image.width : undefined;
      const height = typeof image.height === "number" ? image.height : undefined;
      const imageId = typeof image.image_id === "string" ? image.image_id : undefined;
      blocks.push({
        type: "image",
        src,
        mediaType,
        alt,
        ...(width !== undefined ? { width } : {}),
        ...(height !== undefined ? { height } : {}),
        ...(blobId ? { blobId } : {}),
        ...(imageId ? { imageId } : {}),
      });
    }
  }
  return blocks;
}

function peerLastSegment(value: string): string {
  return value.split("/").pop() || value;
}

function summarizePeersResult(result: unknown): string | null {
  let parsed = typeof result === "string"
    ? parseJsonPayload(result)
    : result && typeof result === "object"
      ? result
      : null;
  if (typeof parsed === "string") {
    parsed = parseJsonPayload(parsed);
  }
  if (!parsed || typeof parsed !== "object") return null;
  const peers = (parsed as Record<string, unknown>).peers;
  if (!Array.isArray(peers)) return null;
  const roleCounts = new Map<string, number>();
  const preview: string[] = [];
  for (const peer of peers) {
    if (!peer || typeof peer !== "object") continue;
    const record = peer as Record<string, unknown>;
    const rawName = typeof record.name === "string" && record.name.trim()
      ? record.name.trim()
      : typeof (record.address as Record<string, unknown> | undefined)?.endpoint === "string"
        ? String((record.address as Record<string, unknown>).endpoint).trim()
        : typeof record.peer_id === "string"
          ? record.peer_id.trim()
          : "";
    if (!rawName) continue;
    const parts = rawName.split("/").filter(Boolean);
    const role = parts.length >= 2 ? parts[parts.length - 2] : "peer";
    roleCounts.set(role, (roleCounts.get(role) || 0) + 1);
    if (preview.length < 8) preview.push(peerLastSegment(rawName));
  }
  const roles = [...roleCounts.entries()]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .slice(0, 5)
    .map(([role, count]) => `${role} ${count}`)
    .join(", ");
  const lines = [`${peers.length} peers${roles ? ` · ${roles}` : ""}`];
  if (preview.length > 0) {
    lines.push(`First peers: ${preview.join(", ")}`);
  }
  return lines.join("\n");
}

function summarizeToolResultForDisplay(toolName: string | undefined, result: unknown): string | null {
  if (toolName === "peers") {
    const summary = summarizePeersResult(result);
    if (summary) return summary;
  }
  return null;
}

function formatServerToolAnnotations(annotations: unknown[]): string {
  return annotations
    .map((annotation, index) => {
      const record = annotation && typeof annotation === "object"
        ? annotation as Record<string, unknown>
        : null;
      const title = typeof record?.title === "string" && record.title.trim()
        ? record.title.trim()
        : typeof record?.text === "string" && record.text.trim()
          ? record.text.trim()
          : `Source ${index + 1}`;
      const url = typeof record?.url === "string" && record.url.trim()
        ? record.url.trim()
        : "";
      return url ? `${index + 1}. ${title}\n${url}` : `${index + 1}. ${title}`;
    })
    .join("\n\n")
    .trim();
}

function serverToolContentSummary(frame: ConsoleFrame): { result?: string; status: "pending" | "success" | "error" } | null {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  const content = record?.content && typeof record.content === "object"
    ? record.content as Record<string, unknown>
    : null;
  const type = typeof content?.type === "string"
    ? content.type
    : typeof record?.type === "string"
      ? record.type
      : "";
  const status = typeof content?.status === "string"
    ? content.status
    : typeof record?.status === "string"
      ? record.status
      : "";
  if (
    type.includes(".failed") ||
    type.includes(".error") ||
    status === "failed" ||
    status === "error"
  ) {
    return { status: "error" };
  }
  if (Array.isArray(content?.annotations)) {
    const result = formatServerToolAnnotations(content.annotations);
    return {
      status: "success",
      ...(result ? { result } : {}),
    };
  }
  if (
    type.includes(".completed") ||
    type.includes(".done") ||
    status === "completed" ||
    status === "done" ||
    status === "succeeded"
  ) {
    return { status: "success" };
  }
  if (
    type.includes(".in_progress") ||
    type.includes(".searching") ||
    type.includes(".started") ||
    status === "in_progress" ||
    status === "searching" ||
    status === "queued" ||
    type.includes("_call")
  ) {
    return { status: "pending" };
  }
  return null;
}

function isActiveServerToolContentFrame(frame: ConsoleFrame): boolean {
  return serverToolContentSummary(frame)?.status === "pending";
}

function isTerminalServerToolContentFrame(frame: ConsoleFrame): boolean {
  const record = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : null;
  const content = record?.content && typeof record.content === "object"
    ? record.content as Record<string, unknown>
    : null;
  const type = typeof content?.type === "string" ? content.type : "";
  if (type === "message_annotations" || Array.isArray(content?.annotations)) return false;
  const status = serverToolContentSummary(frame)?.status;
  return status === "success" || status === "error";
}

type HistoryToolResult = {
  result?: string;
  images?: ConversationRichImageBlock[];
  status: "pending" | "success" | "error";
  completionEvidence: ToolCompletionEvidence;
};

function isToolResultImageBlock(block: unknown): boolean {
  if (!block || typeof block !== "object") return false;
  const type = (block as Record<string, unknown>).type;
  return type === "image" || type === "image_ref";
}

/** Split a content-block list into the image blocks presented as images
 * (inline as data URLs, stored through `/blobs/{id}`) and the residual
 * content, in order. An image block that cannot be presented stays in the
 * residual. Anything but a list is all residual. */
function splitToolResultImages(
  content: unknown,
  blobBaseUrl?: string,
): { images: ConversationRichImageBlock[]; residual: unknown } {
  if (!Array.isArray(content) || !content.some(isToolResultImageBlock)) {
    return { images: [], residual: content };
  }
  const images: ConversationRichImageBlock[] = [];
  const residual: unknown[] = [];
  for (const block of content) {
    const presented = isToolResultImageBlock(block)
      ? contentToUserBlocks([block], blobBaseUrl, "legacy")
        .filter((item): item is ConversationRichImageBlock => item.type === "image")
      : [];
    if (presented.length > 0) images.push(...presented);
    else residual.push(block);
  }
  return { images, residual };
}

/** A tool result's text and images. The text carrier is `result`, else
 * `content`; its presented images are removed and the residual keeps the
 * canonical text-only/JSON formatting. Images come from that carrier, else
 * from the content beside a text `result`. */
function toolResultTextAndImages(
  record: Record<string, unknown> | null | undefined,
  blobBaseUrl?: string,
): { text?: string; images: ConversationRichImageBlock[] } {
  const carrier = record?.result ?? record?.content;
  const split = splitToolResultImages(carrier, blobBaseUrl);
  const images = split.images.length > 0 || carrier === record?.content
    ? split.images
    : splitToolResultImages(record?.content, blobBaseUrl).images;
  const residualEmpty = split.images.length > 0
    && Array.isArray(split.residual) && split.residual.length === 0;
  const text = residualEmpty ? undefined : toolResultTextFromContent(split.residual);
  return { ...(text !== undefined ? { text } : {}), images };
}

function toolResultTextFromContent(content: unknown): string | undefined {
  if (typeof content === "string") return content;
  if (content === undefined || content === null) return undefined;
  // Flatten only the canonical text-only carrier. Mixed blocks, annotations,
  // resources and arbitrary structured results retain their complete payload.
  if (Array.isArray(content) && content.length > 0 && content.every((block) => {
    if (!block || typeof block !== "object") return false;
    const record = block as Record<string, unknown>;
    return record.type === "text" && typeof record.text === "string"
      && Object.keys(record).every((key) => key === "type" || key === "text");
  })) {
    return content.map((block) => (block as { text: string }).text).join("");
  }
  return JSON.stringify(content, null, 2);
}

function historyToolResults(
  frames: ConsoleFrame[],
  includeLive = false,
  blobBaseUrl?: string,
): Map<string, HistoryToolResult> {
  const results = new Map<string, HistoryToolResult>();
  for (const frame of frames) {
    if (
      (!includeLive && frame.sourceKind !== "session_history")
      || (frame.event !== "tool_execution_completed" && frame.event !== "tool_result_received" && frame.event !== "tool_execution_timed_out")
    ) {
      continue;
    }
    const data = frame.data && typeof frame.data === "object"
      ? frame.data as Record<string, unknown>
      : null;
    const toolCallId = typeof data?.tool_call_id === "string" && data.tool_call_id.trim()
      ? data.tool_call_id.trim()
      : typeof data?.id === "string" && data.id.trim()
        ? data.id.trim()
        : "";
    if (!toolCallId) continue;
    const feedback = operationFeedbackFromFrame(frame);
    const { text, images } = toolResultTextAndImages(data, blobBaseUrl);
    const result = feedback ? feedback.detail : text;
    const completionEvidence = toolCompletionFromFrame(frame, toolCallId);
    const status = completionEvidence.outcome === "success" ? "success" : completionEvidence.outcome === "error" ? "error" : "pending";
    const next: HistoryToolResult = {
      status, completionEvidence,
      ...(result !== undefined ? { result } : {}),
      ...(images.length > 0 ? { images } : {}),
    };
    const previous = results.get(toolCallId);
    results.set(toolCallId, previous ? { ...previous, ...laterToolResult(previous, next) } : next);
  }
  return results;
}

function blockAssistantToolBlocks(
  blocks: unknown[],
  peerRegistry?: Map<string, string>,
  toolResults?: Map<string, HistoryToolResult>,
): ConversationRichToolCallBlock[] {
  const toolBlocks: ConversationRichToolCallBlock[] = [];
  let toolIndex = 0;
  for (const block of blocks) {
    if (!block || typeof block !== "object") continue;
    const item = block as Record<string, unknown>;
    const toolBlock = blockAssistantToolBlock(item, toolIndex, peerRegistry, toolResults);
    if (!toolBlock) continue;
    toolIndex += 1;
    toolBlocks.push(toolBlock);
  }
  return toolBlocks;
}

function blockAssistantToolBlock(
  item: Record<string, unknown>,
  index: number,
  peerRegistry?: Map<string, string>,
  toolResults?: Map<string, HistoryToolResult>,
): ConversationRichToolCallBlock | null {
    const blockType = typeof item.block_type === "string"
      ? item.block_type
      : typeof item.type === "string"
        ? item.type
        : "";
    if (blockType !== "tool_use") return null;
    const data = item.data && typeof item.data === "object"
      ? item.data as Record<string, unknown>
      : item;
    const name = typeof data.name === "string" && data.name.trim()
      ? data.name.trim()
      : "tool";
    const id = typeof data.id === "string" && data.id.trim()
      ? data.id.trim()
      : `history-tool-${index + 1}`;
    const args = data.args !== undefined ? data.args : data.arguments;
    const argsRecord = args && typeof args === "object" ? args as Record<string, unknown> : null;
    const argumentsText = args === undefined
      ? ""
      : typeof args === "string"
        ? args
        : JSON.stringify(args);
    const isPeerTool = name === "send_request" || name === "send_message" || name === "send_response";
    const peerTarget = isPeerTool ? peerTargetFromArgs(argsRecord, peerRegistry) : undefined;
    const rawPeerIntent = isPeerTool && typeof argsRecord?.intent === "string"
      ? argsRecord.intent
      : undefined;
    const peerIntent = displayPeerIntent(rawPeerIntent);
    const peerBody = isPeerTool ? extractPeerBodyFromArgs(argsRecord) : undefined;
    const result = toolResults?.get(id);
    const displayResult = result?.result !== undefined
      ? summarizeToolResultForDisplay(name, result.result) ?? result.result
      : undefined;
    return {
      type: "tool-call",
      toolCallId: id,
      name,
      arguments: argumentsText,
      ...(displayResult !== undefined ? { result: displayResult } : {}),
      ...(result?.images ? { images: result.images } : {}),
      status: result?.status || "pending",
      completionEvidence: result?.completionEvidence ?? unknownToolCompletion(id),
      ...(peerTarget ? { peerTarget } : {}),
        ...(isPeerTool ? { peerIdentity: typeof argsRecord?.peer_id === "string" ? argsRecord.peer_id : typeof argsRecord?.to === "string" ? argsRecord.to : "Unknown peer" } : {}),
        ...(isPeerTool && typeof argsRecord?.peer_id === "string" && peerRegistry?.get(argsRecord.peer_id)?.trim()
          ? { peerDisplayLabel: decodeMemberAlias(peerLastSegment(peerRegistry.get(argsRecord.peer_id)!.trim())) } : {}),
      ...(peerIntent ? { peerIntent } : {}),
      ...(peerBody ? { peerBody, peerBodyFormat: "verbatim" as const } : {}),
    };
}

function blockAssistantRichBlocks(
  blocks: unknown[],
  peerRegistry?: Map<string, string>,
  toolResults?: Map<string, HistoryToolResult>,
  textMode: ConversationTextMode = "legacy",
  blobBaseUrl?: string,
): ConversationRichBlock[] {
  const serverFrames = new Map<unknown, ConsoleFrame>();
  for (const value of blocks) {
    if (!value || typeof value !== "object") continue;
    const block = value as Record<string, unknown>;
    if ((block.block_type ?? block.type) !== "server_tool_content") continue;
    serverFrames.set(value, { id: "", event: "server_tool_content", data: block.data } as ConsoleFrame);
  }
  const serverBlocks = buildToolBlocks([...serverFrames.values()], blobBaseUrl);
  const emittedServerTools = new Set<string>();
  const actionAndTextBlocks: ConversationRichBlock[] = [];
  let hasNonTextBlock = false;
  let toolIndex = 0;
  for (const block of blocks) {
    if (!block || typeof block !== "object") continue;
    const item = block as Record<string, unknown>;
    const blockType = typeof item.block_type === "string"
      ? item.block_type
      : typeof item.type === "string"
        ? item.type
        : "";
    const data = item.data && typeof item.data === "object"
      ? item.data as Record<string, unknown>
      : {};
    if (blockType === "reasoning") {
      const text = reasoningBlockText(item);
      if (text) {
        hasNonTextBlock = true;
        actionAndTextBlocks.push({
          type: "thinking",
          label: "",
          text,
          final: true,
          persisted: true,
        });
      }
      continue;
    }
    if (blockType === "tool_use") {
      const toolBlock = blockAssistantToolBlock(item, toolIndex, peerRegistry, toolResults);
      toolIndex += 1;
      if (toolBlock) {
        hasNonTextBlock = true;
        actionAndTextBlocks.push(toolBlock);
      }
      continue;
    }
    if (blockType === "image") {
      const imageFrame = { id: "", event: "assistant_image", data: { image: data } } as ConsoleFrame;
      const imageEntry = renderAssistantImageEntry(null, imageFrame, "", blobBaseUrl);
      if (imageEntry?.kind === "message" && imageEntry.blocks) {
        hasNonTextBlock = true;
        actionAndTextBlocks.push(...imageEntry.blocks);
      }
      continue;
    }
    if (blockType === "server_tool_content") {
      const serverFrame = serverFrames.get(block)!;
      const id = parseToolCallId(serverFrame), tool = id ? serverBlocks.get(id) : undefined;
      if (tool && !emittedServerTools.has(tool.toolCallId)) {
        emittedServerTools.add(tool.toolCallId);
        hasNonTextBlock = true;
        actionAndTextBlocks.push(tool);
      } else if (!id) {
        const result = serverToolContentSummary(serverFrame)?.result;
        if (result && ![...serverBlocks.values()].some(tool => tool.result === result)) {
          hasNonTextBlock = true;
          actionAndTextBlocks.push({ type: "paragraph", text: result });
        }
      }
      continue;
    }
    if (blockType === "text" || blockType === "transcript") {
      const text = typeof data.text === "string"
        ? data.text
        : typeof item.text === "string"
          ? item.text
          : "";
      if (textMode === "markdown" && text) {
        const previous = actionAndTextBlocks.at(-1);
        if (previous?.type === "markdown") previous.source += text;
        else actionAndTextBlocks.push(...messageTextBlocks(text, textMode));
      } else if (textMode !== "markdown" && text.trim()) {
        actionAndTextBlocks.push(...messageTextBlocks(text, textMode));
      }
    }
  }
  return hasNonTextBlock || textMode === "markdown" ? actionAndTextBlocks : [];
}

function textFromUnknown(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function typedNoticeContentBlocks(content: unknown, blobBaseUrl?: string): ConversationRichBlock[] {
  return contentToUserBlocks(content, blobBaseUrl, "legacy");
}

function typedNoticeBlockText(block: Record<string, unknown>): string {
  const parts = [
    textFromUnknown(block.summary),
    textFromUnknown(block.body),
    textFromUnknown(block.detail),
    textFromUnknown(block.state),
    textFromUnknown(block.status),
  ].filter(Boolean);
  return parts.join("\n");
}

function typedCommsStableBodyText(block: Record<string, unknown>): string {
  const parts = [
    textFromUnknown(block.summary),
    textFromUnknown(block.body),
    textFromUnknown(block.detail),
  ].filter(Boolean);
  return parts.join("\n");
}

function stripCommsIntentBodyPrefix(text: string, peerAliases: string[] = []): string {
  const match = text.match(/^\s*\[COMMS\s+(?:MESSAGE|REQUEST|RESPONSE)\s+from\s+([^\]\n]+)\]\s*\n\s*Intent:\s*[^\n]*\n\s*Body:\s*([\s\S]+)$/i)
    || text.match(/^\s*Peer\s+(?:message|request|response)\s+from\s+(.+):\s*\n\s*Intent:\s*[^\n]*\n\s*Body:\s*([\s\S]+)$/i);
  if (match?.[1] && peerAliases.length > 0) {
    const peer = normalizePeerAlias(match[1]);
    if (!peerAliases.includes(peer)) return text.trim();
  }
  return (match?.[2] || text).trim();
}

function stripBareCommsIntentBodyPrefix(text: string): string {
  const match = text.match(/^\s*Intent:\s*[^\n]*\n\s*Body:\s*([\s\S]+)$/i);
  return (match?.[1] || text).trim();
}

function isExternalEventOnlySystemNotice(message: unknown): boolean {
  if (!message || typeof message !== "object") return false;
  const record = message as Record<string, unknown>;
  if (textFromUnknown(record.kind) === "external_event") return true;
  const blocks = record.blocks;
  if (!Array.isArray(blocks)) return false;
  let sawExternalEventBlock = false;
  for (const block of blocks) {
    if (!block || typeof block !== "object") continue;
    const type = textFromUnknown((block as Record<string, unknown>).type);
    if (!type) continue;
    if (type !== "external_event") return false;
    sawExternalEventBlock = true;
  }
  return sawExternalEventBlock;
}

function systemNoticeMessageRecord(frame: ConsoleFrame): Record<string, unknown> | null {
  if (frame.event !== "system_notice" || !frame.data || typeof frame.data !== "object") {
    return null;
  }
  const data = frame.data as Record<string, unknown>;
  if (data.message && typeof data.message === "object") {
    return data.message as Record<string, unknown>;
  }
  return data;
}

function commsNoticeMessageRecord(frame: ConsoleFrame): Record<string, unknown> | null {
  const systemNotice = systemNoticeMessageRecord(frame);
  if (systemNotice) return systemNotice;
  if (frame.sourceKind !== "session_history" || !frame.data || typeof frame.data !== "object") {
    return null;
  }
  if (
    frame.event !== "text_complete"
    && frame.event !== "interaction_complete"
    && frame.event !== "interaction_failed"
    && frame.event !== "run_failed"
  ) {
    return null;
  }
  const message = (frame.data as Record<string, unknown>).message;
  if (!message || typeof message !== "object") return null;
  const record = message as Record<string, unknown>;
  return textFromUnknown(record.role) === "system_notice" ? record : null;
}

function systemNoticeBlockRecords(record: Record<string, unknown>): Record<string, unknown>[] {
  const blocks = record.blocks;
  if (!Array.isArray(blocks)) return [];
  return blocks.filter((block): block is Record<string, unknown> => (
    Boolean(block) && typeof block === "object"
  ));
}

function legacyPeerNoticeTextCandidates(record: Record<string, unknown>): string[] {
  const candidates: string[] = [];
  const body = textFromUnknown(record.body).trim();
  if (body) candidates.push(body);
  for (const block of systemNoticeBlockRecords(record)) {
    const blockText = typedNoticeBlockText(block).trim();
    if (blockText) candidates.push(blockText);
    const content = block.content;
    if (!Array.isArray(content)) continue;
    for (const item of content) {
      if (!item || typeof item !== "object") continue;
      const itemRecord = item as Record<string, unknown>;
      const itemText = textFromUnknown(itemRecord.text).trim();
      if (itemText) candidates.push(itemText);
      const data = itemRecord.data;
      if (data && typeof data === "object") {
        const dataText = textFromUnknown((data as Record<string, unknown>).text).trim();
        if (dataText) candidates.push(dataText);
      }
    }
  }
  return candidates;
}

function isLegacyPeerNoticeText(text: string): boolean {
  return /^(Peer (?:message|request|response) from|\[COMMS (?:MESSAGE|REQUEST|RESPONSE)\b)/i.test(text.trim());
}

function isCommsLikeRunStartedPrompt(text: string): boolean {
  const trimmed = text.trim();
  return /(^|\n)\s*Peer (?:message|request|response)(?:\s+from\b|$)/i.test(trimmed)
    || /(^|\n)\s*\[COMMS (?:MESSAGE|REQUEST|RESPONSE)\b/i.test(trimmed);
}

const PEER_ENVELOPE_LINE_RE = /^Peer\s+(?:message|request|response)\s+from\s+(.+):(.*)$/i;
const BRACKETED_COMMS_LINE_RE = /^\[COMMS\s+(?:MESSAGE|REQUEST|RESPONSE)\s+from\s+([^\]]+)\](.*)$/i;

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function stripPeerEnvelopeByAlias(text: string, peerAliases: string[]): string | null {
  const normalized = text.replace(/\r/g, "\n").trim();
  const aliases = [...peerAliases].filter(Boolean).sort((a, b) => b.length - a.length);
  for (const alias of aliases) {
    const escaped = escapeRegExp(alias);
    const peerEnvelope = new RegExp(
      `^Peer\\s+(?:message|request|response)\\s+from\\s+${escaped}:(?:\\s+|$)`,
      "i",
    );
    const bracketedEnvelope = new RegExp(
      `^\\[COMMS\\s+(?:MESSAGE|REQUEST|RESPONSE)\\s+from\\s+${escaped}\\]\\s*`,
      "i",
    );
    const lines = normalized.split("\n").map((line) => line.trim());
    for (let index = 0; index < lines.length; index++) {
      const line = lines[index];
      if (!line || /^Peer (?:message|request|response)$/i.test(line)) continue;
      const peerMatch = line.match(peerEnvelope);
      const bracketMatch = line.match(bracketedEnvelope);
      if (!peerMatch && !bracketMatch) return null;
      const bodyOnEnvelopeLine = line.replace(peerEnvelope, "").replace(bracketedEnvelope, "").trim();
      const bodyLines = [
        ...(bodyOnEnvelopeLine ? [bodyOnEnvelopeLine] : []),
        ...lines
          .slice(index + 1)
          .filter((candidate) => !isPeerEnvelopeScaffoldLine(candidate, aliases, true)),
      ];
      return bodyLines.join("\n").trim();
    }
  }
  return null;
}

function isPeerEnvelopeScaffoldLine(
  line: string,
  peerAliases: string[] = [],
  allowStandaloneScaffold = false,
): boolean {
  if (!line) return true;
  if (allowStandaloneScaffold && /^Peer (?:message|request|response)$/i.test(line)) return true;
  if (
    peerAliases.length === 0
    && /^\[COMMS (?:MESSAGE|REQUEST|RESPONSE) from [^\]]+\]$/i.test(line)
  ) return true;
  for (const alias of peerAliases) {
    if (!alias) continue;
    const escaped = escapeRegExp(alias);
    if (new RegExp(`^Peer\\s+(?:message|request|response)\\s+from\\s+${escaped}:\\s*$`, "i").test(line)) {
      return true;
    }
    if (new RegExp(`^\\[COMMS\\s+(?:MESSAGE|REQUEST|RESPONSE)\\s+from\\s+${escaped}\\]\\s*$`, "i").test(line)) {
      return true;
    }
  }
  return false;
}

function isImagePlaceholderLine(line: string): boolean {
  return /^\[image:\s*[^\]]+\]$/i.test(line.trim());
}

function normalizePeerEnvelopeText(text: string, peerAliases: string[] = []): string {
  const allowGenericEnvelopeStrip = peerAliases.length === 0;
  const intentBodyStripped = stripCommsIntentBodyPrefix(text, peerAliases);
  const envelopeStripped = intentBodyStripped === text.trim()
    ? stripPeerEnvelopeByAlias(text, peerAliases)
    : null;
  const aliasStripped = envelopeStripped ?? (
    intentBodyStripped === text.trim() ? null : intentBodyStripped
  );
  let normalized = (envelopeStripped !== null
    ? stripBareCommsIntentBodyPrefix(envelopeStripped)
    : aliasStripped ?? text)
    .replace(/\r/g, "\n")
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => {
      if (
        isPeerEnvelopeScaffoldLine(
          line,
          peerAliases,
          allowGenericEnvelopeStrip || aliasStripped !== null,
        )
      ) return false;
      if (isImagePlaceholderLine(line)) return false;
      if (
        allowGenericEnvelopeStrip
        && PEER_ENVELOPE_LINE_RE.test(line)
        && !line.replace(PEER_ENVELOPE_LINE_RE, "$2").trim()
      ) return false;
      return true;
    })
    .join("\n")
  if (allowGenericEnvelopeStrip) {
    normalized = normalized
      .replace(PEER_ENVELOPE_LINE_RE, "$2")
      .replace(/^\[COMMS\s+(?:MESSAGE|REQUEST|RESPONSE)\s+from\s+[^\]]+\]\s*/i, "");
  }
  return normalized.replace(/\s+/g, " ").trim();
}

function normalizeStructuredCommsBodyText(text: string, peerAliases: string[] = []): string {
  const trimmed = text.trim();
  if (trimmed) {
    const stripped = stripPeerEnvelopeByAlias(trimmed, peerAliases);
    if (stripped !== null) return stripped.replace(/\s+/g, " ").trim();
  }
  return trimmed
    .replace(/\r/g, "\n")
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .join(" ");
}

function normalizePeerAlias(value: string): string {
  return value.trim().toLowerCase();
}

function peerFromCommsText(text: string): string {
  const trimmed = text.trim();
  const peerLine = trimmed.match(/(?:^|\n)\s*Peer\s+(?:message|request|response)\s+from\s+(.+):(?:[^\n]*)/i);
  if (peerLine?.[1]) return normalizePeerAlias(peerLine[1]);
  const bracketed = trimmed.match(/(?:^|\n)\s*\[COMMS\s+(?:MESSAGE|REQUEST|RESPONSE)\s+from\s+([^\]]+)\]/i);
  if (bracketed?.[1]) return normalizePeerAlias(bracketed[1]);
  return "";
}

type StructuredCommsNoticeSignature = {
  peer: string;
  peerAliases: string[];
  body: string;
  kind?: string;
  direction?: string;
  occurrenceId?: string;
  timestampMs?: number;
  sourceIndex?: number;
  sourceKind?: string;
  consumed?: boolean;
};

const STRUCTURED_COMMS_PROMPT_MATCH_WINDOW_MS = 30_000;

function normalizedPeerAliases(...values: string[]): string[] {
  const aliases: string[] = [];
  for (const value of values) {
    const alias = normalizePeerAlias(value);
    if (alias && !aliases.includes(alias)) aliases.push(alias);
  }
  return aliases;
}

function commsKindFromText(text: string): string {
  const match = text.trim().match(/(?:^|\n)\s*Peer\s+(message|request|response)\s+from\s+/i)
    || text.trim().match(/(?:^|\n)\s*\[COMMS\s+(MESSAGE|REQUEST|RESPONSE)\s+from\s+/i);
  return match?.[1]?.toLowerCase() || "";
}

function systemNoticeCommsSignatures(frame: ConsoleFrame): StructuredCommsNoticeSignature[] {
  const record = commsNoticeMessageRecord(frame);
  if (!record || isExternalEventOnlySystemNotice(record)) return [];
  const isCommsNotice = textFromUnknown(record.kind) === "comms"
    || systemNoticeBlockRecords(record).some((block) => textFromUnknown(block.type) === "comms")
    || (canUseLegacyPeerNoticeText(record)
      && legacyPeerNoticeTextCandidates(record).some(isLegacyPeerNoticeText));
  if (!isCommsNotice) return [];

  const signatures: StructuredCommsNoticeSignature[] = [];
  const seenSignatures = new Set<string>();
  const pushCandidate = (
    candidate: string,
    peerAliases: string[] = [],
    occurrenceId?: string,
    kind?: string,
    direction?: string,
  ) => {
    const aliases = peerAliases.length ? peerAliases : normalizedPeerAliases(peerFromCommsText(candidate));
    const body = normalizePeerEnvelopeText(candidate, aliases);
    if (!body) return;
    const candidateKind = kind || commsKindFromText(candidate);
    const candidateDirection = direction || (commsKindFromText(candidate) ? "incoming" : "");
    const key = [
      aliases.join("|"),
      body,
      candidateKind,
      candidateDirection,
      occurrenceId || "",
    ].join("\u0000");
    if (seenSignatures.has(key)) return;
    seenSignatures.add(key);
    signatures.push({
      peer: aliases[0] || "",
      peerAliases: aliases,
      body,
      kind: candidateKind,
      direction: candidateDirection,
      occurrenceId,
      timestampMs: frame.timestampMs,
      sourceKind: frame.sourceKind,
    });
  };

  const noticeOccurrenceId = textFromUnknown(record.request_id)
    || textFromUnknown(record.correlation_id)
    || textFromUnknown(record.id);
  const noticeBlocks = systemNoticeBlockRecords(record);
  const typedCommsBlocks = noticeBlocks
    .filter((block) => textFromUnknown(block.type) === "comms");
  if (!typedCommsBlocks.length) {
    for (const candidate of legacyPeerNoticeTextCandidates(record)) {
      pushCandidate(candidate, [], noticeOccurrenceId);
    }
  }
  const body = textFromUnknown(record.body);
  if (body && !typedCommsBlocks.length) pushCandidate(body, [], noticeOccurrenceId);
  for (let index = 0; index < typedCommsBlocks.length; index++) {
    const block = typedCommsBlocks[index];
    const peer = block.peer && typeof block.peer === "object"
      ? block.peer as Record<string, unknown>
      : {};
    const peerAliases = normalizedPeerAliases(
      textFromUnknown(peer.display_name),
      textFromUnknown(peer.id),
    );
    const blockOccurrenceId = textFromUnknown(block.request_id)
      || textFromUnknown(block.correlation_id)
      || textFromUnknown(block.id)
      || (noticeOccurrenceId ? `${noticeOccurrenceId}:${index}` : `${index}`);
    const blockKind = textFromUnknown(block.kind);
    const blockDirection = textFromUnknown(block.direction);
    const contentText = typedNoticeContentBlocks(block.content)
      .map((item) => item.type === "paragraph" ? item.text : "")
      .filter(Boolean)
      .join("\n");
    const stableBodyText = typedCommsStableBodyText(block);
    const candidateText = contentText || stableBodyText || body;
    if (candidateText) pushCandidate(candidateText, peerAliases, blockOccurrenceId, blockKind, blockDirection);
  }
  return signatures;
}

function structuredCommsNoticeTextSignatures(frames: ConsoleFrame[]): StructuredCommsNoticeSignature[] {
  const signatures: StructuredCommsNoticeSignature[] = [];
  const seen = new Set<string>();
  for (let index = 0; index < frames.length; index++) {
    const frame = frames[index];
    for (const signature of systemNoticeCommsSignatures(frame)) {
      const primaryPeerAlias = signature.peer || signature.peerAliases[0] || "";
      const key = [
        frame.id || `${frame.event}:${index}`,
        primaryPeerAlias,
        signature.kind || "",
        signature.direction || "",
        signature.occurrenceId || "",
        signature.body,
      ].join("\u0000");
      if (seen.has(key)) continue;
      seen.add(key);
      signatures.push({
        ...signature,
        sourceIndex: index,
      });
    }
  }
  return signatures;
}

function runStartedPromptMatchesStructuredCommsNotice(
  frame: ConsoleFrame,
  signature: StructuredCommsNoticeSignature,
): boolean {
  if (frame.event !== "run_started" || typeof frame.data !== "object" || frame.data === null) {
    return false;
  }
  const prompt = extractPromptText((frame.data as Record<string, unknown>).prompt).trim();
  if (!prompt || !isCommsLikeRunStartedPrompt(prompt)) return false;
  const promptPeer = peerFromCommsText(prompt);
  const promptKind = commsKindFromText(prompt);
  const normalizedPrompt = normalizePeerEnvelopeText(prompt, signature.peerAliases);
  if (!normalizedPrompt) return false;
  if (promptKind && (!signature.kind || promptKind !== signature.kind)) return false;
  if (signature.direction === "outgoing") return false;
  const matchedByAlias = stripPeerEnvelopeByAlias(prompt, signature.peerAliases) !== null;
  if (promptPeer && signature.peerAliases.length === 0) return false;
  if (
    promptPeer
    && signature.peerAliases.length > 0
    && !signature.peerAliases.includes(promptPeer)
    && !matchedByAlias
  ) {
    return false;
  }
  if (
    typeof frame.timestampMs === "number"
    && typeof signature.timestampMs === "number"
    && Math.abs(frame.timestampMs - signature.timestampMs) > STRUCTURED_COMMS_PROMPT_MATCH_WINDOW_MS
  ) {
    return false;
  }
  return Boolean(signature.body && normalizedPrompt === signature.body);
}

function runStartedPromptHasImagePlaceholder(frame: ConsoleFrame): boolean {
  if (frame.event !== "run_started" || typeof frame.data !== "object" || frame.data === null) {
    return false;
  }
  const prompt = extractPromptText((frame.data as Record<string, unknown>).prompt);
  return prompt
    .replace(/\r/g, "\n")
    .split("\n")
    .some(isImagePlaceholderLine);
}

function structuredCommsPromptSuppressionKeys(
  frames: ConsoleFrame[],
  structuredCommsSignatures: StructuredCommsNoticeSignature[],
): Set<string> {
  const keys = new Set<string>();
  const consumed = new Set<string>();
  const consumedStructuredNotices = new Set<string>();
  for (const signature of structuredCommsSignatures) {
    const signatureKey = [
      signature.sourceIndex ?? "",
      signature.peerAliases.join("|"),
      signature.body,
      signature.kind || "",
      signature.direction || "",
      signature.occurrenceId || "",
    ].join("\u0000");
    if (consumedStructuredNotices.has(signatureKey)) continue;
    consumedStructuredNotices.add(signatureKey);
    let best: { key: string; distance: number } | null = null;
    for (let index = 0; index < frames.length; index++) {
      const frame = frames[index];
      const key = frame.id || `${frame.event || "frame"}:${index}`;
      if (consumed.has(key)) continue;
      if (
        typeof signature.timestampMs === "number"
        && typeof frame.timestampMs === "number"
      ) {
        if (frame.timestampMs > signature.timestampMs && !runStartedPromptHasImagePlaceholder(frame)) {
          continue;
        }
        if (
          frame.timestampMs === signature.timestampMs
          && typeof signature.sourceIndex === "number"
          && index > signature.sourceIndex
        ) continue;
      } else if (typeof signature.sourceIndex === "number" && index > signature.sourceIndex) {
        continue;
      }
      if (!runStartedPromptMatchesStructuredCommsNotice(frame, signature)) continue;
      const distance = typeof frame.timestampMs === "number" && typeof signature.timestampMs === "number"
        ? Math.abs(frame.timestampMs - signature.timestampMs)
        : Math.abs(index - (signature.sourceIndex ?? index));
      if (!best || distance < best.distance) {
        best = {
          key,
          distance,
        };
      }
    }
    if (best) {
      keys.add(best.key);
      consumed.add(best.key);
    }
  }
  return keys;
}

function commsNoticeDedupeKeys(frame: ConsoleFrame): string[] {
  if (runtimeAppendNoticeKey(frame)) return [];
  const signatures = systemNoticeCommsSignatures(frame);
  const keys: string[] = [];
  for (const signature of signatures) {
    if (!signature.body) continue;
    const key = [
      signature.peer || "unknown",
      signature.kind || "message",
      signature.direction || "incoming",
      signature.occurrenceId || "",
      signature.body,
    ].join(":");
    if (!keys.includes(key)) keys.push(key);
  }
  return keys;
}

function commsNoticeDuplicateKey(
  key: string,
  frame: ConsoleFrame,
  emitted: Map<string, { sourceKind?: string; timestampMs?: number }>,
): boolean {
  const previous = emitted.get(key);
  if (!previous) return false;
  const sourceKind = frame.sourceKind || "live";
  const previousSourceKind = previous.sourceKind || "live";
  const mixedLiveHistory = sourceKind !== previousSourceKind
    && (sourceKind === "session_history" || previousSourceKind === "session_history");
  const closeInTime = typeof frame.timestampMs !== "number"
    || typeof previous.timestampMs !== "number"
    || Math.abs(frame.timestampMs - previous.timestampMs) <= 60_000;
  return mixedLiveHistory && closeInTime;
}

function markCommsNoticeDedupeKey(
  key: string,
  frame: ConsoleFrame,
  emitted: Map<string, { sourceKind?: string; timestampMs?: number }>,
): void {
  emitted.set(key, { sourceKind: frame.sourceKind, timestampMs: frame.timestampMs });
}

function commsNoticeDedupeKeysFromBlock(
  record: Record<string, unknown>,
  fallbackBody: string,
  index: number,
): string[] {
  const type = textFromUnknown(record.type);
  const keys: string[] = [];
  const pushKey = (
    candidate: string,
    peerAliases: string[] = [],
    occurrenceId?: string,
    kind?: string,
    direction?: string,
  ) => {
    const aliases = peerAliases.length ? peerAliases : normalizedPeerAliases(peerFromCommsText(candidate));
    const body = normalizePeerEnvelopeText(candidate, aliases);
    if (!body) return;
    const candidateKind = kind || commsKindFromText(candidate) || "message";
    const candidateDirection = direction || (commsKindFromText(candidate) ? "incoming" : "incoming");
    const key = [
      aliases[0] || "unknown",
      candidateKind,
      candidateDirection,
      occurrenceId || "",
      body,
    ].join(":");
    if (!keys.includes(key)) keys.push(key);
  };

  if (type === "comms") {
    const peer = record.peer && typeof record.peer === "object"
      ? record.peer as Record<string, unknown>
      : {};
    const peerAliases = normalizedPeerAliases(
      textFromUnknown(peer.display_name),
      textFromUnknown(peer.id),
    );
    const contentText = typedNoticeContentBlocks(record.content)
      .map((item) => item.type === "paragraph" ? item.text : "")
      .filter(Boolean)
      .join("\n");
    const stableBodyText = typedCommsStableBodyText(record);
    const occurrenceId = textFromUnknown(record.request_id)
      || textFromUnknown(record.correlation_id)
      || textFromUnknown(record.id)
      || `${index}`;
    pushKey(
      contentText || stableBodyText || fallbackBody,
      peerAliases,
      occurrenceId,
      textFromUnknown(record.kind) || "message",
      textFromUnknown(record.direction) || "incoming",
    );
    return keys;
  }

  if (type && type !== "text") return keys;
  const blockText = typedNoticeBlockText(record).trim();
  if (blockText && isLegacyPeerNoticeText(blockText)) {
    pushKey(blockText);
  }
  const content = record.content;
  if (Array.isArray(content)) {
    for (const item of content) {
      if (!item || typeof item !== "object") continue;
      const itemRecord = item as Record<string, unknown>;
      const itemText = textFromUnknown(itemRecord.text).trim();
      if (itemText && isLegacyPeerNoticeText(itemText)) {
        pushKey(itemText);
      }
      const data = itemRecord.data;
      if (data && typeof data === "object") {
        const dataText = textFromUnknown((data as Record<string, unknown>).text).trim();
        if (dataText && isLegacyPeerNoticeText(dataText)) {
          pushKey(dataText);
        }
      }
    }
  }
  return keys;
}

function consumeCommsNoticeBlockDedupeKeys(
  keys: string[],
  consumeDuplicateCommsBlock?: (key: string) => boolean,
): boolean {
  if (keys.length === 0 || !consumeDuplicateCommsBlock) return false;
  let duplicateCount = 0;
  for (const key of keys) {
    if (consumeDuplicateCommsBlock(key)) duplicateCount += 1;
  }
  return duplicateCount === keys.length;
}

function shouldSuppressDuplicateCommsNotice(
  frame: ConsoleFrame,
  emitted: Map<string, { sourceKind?: string; timestampMs?: number }>,
): boolean {
  const keys = commsNoticeDedupeKeys(frame);
  if (keys.length === 0) return false;
  let duplicateCount = 0;
  for (const key of keys) {
    if (commsNoticeDuplicateKey(key, frame, emitted)) duplicateCount += 1;
  }
  if (duplicateCount === keys.length) {
    return true;
  }
  const record = systemNoticeMessageRecord(frame);
  const hasBlockLevelComms = record
    ? systemNoticeBlockRecords(record).some((block, index) => (
        commsNoticeDedupeKeysFromBlock(block, textFromUnknown(record.body), index).length > 0
      ))
    : false;
  if (!hasBlockLevelComms) {
    for (const key of keys) {
      markCommsNoticeDedupeKey(key, frame, emitted);
    }
  }
  return false;
}

function structuredCommsBodyShouldPreserveLeadingEnvelope(
  body: string,
  peerAliases: string[],
): boolean {
  if (!body.match(/^\s*(?:Peer\s+(?:message|request|response)\s+from\s+.+:|\[COMMS\s+(?:MESSAGE|REQUEST|RESPONSE)\s+from\s+[^\]]+\])\s*\n/i)) {
    return false;
  }
  return peerAliases.some((alias) => alias && !alias.startsWith("implicit-"));
}

function canUseLegacyPeerNoticeText(record: Record<string, unknown>): boolean {
  const kind = textFromUnknown(record.kind);
  if (kind && kind !== "generic") return false;
  const blockTypes = systemNoticeBlockRecords(record)
    .map((block) => textFromUnknown(block.type))
    .filter(Boolean);
  return blockTypes.every((type) => type === "text");
}

export function systemNoticeClearsBusyState(frame: ConsoleFrame): boolean {
  const record = systemNoticeMessageRecord(frame);
  if (!record || isExternalEventOnlySystemNotice(record)) return false;
  if (textFromUnknown(record.kind) === "comms") return true;
  const blocks = systemNoticeBlockRecords(record);
  if (blocks.some((block) => textFromUnknown(block.type) === "comms")) return true;
  if (!canUseLegacyPeerNoticeText(record)) return false;
  return legacyPeerNoticeTextCandidates(record).some(isLegacyPeerNoticeText);
}

function typedSystemNoticeBlocksToRich(
  blocks: unknown,
  body: unknown,
  blobBaseUrl?: string,
  sourceKind?: string,
  consumeDuplicateCommsBlock?: (key: string) => boolean,
  textMode: ConversationTextMode = "legacy",
): ConversationRichBlock[] {
  const rich: ConversationRichBlock[] = [];
  const bodyText = textFromUnknown(body);
  if (!Array.isArray(blocks)) {
    if (bodyText) rich.push({ type: "paragraph", text: bodyText });
    return rich;
  }

  let consumedDuplicateCommsBlock = false;
  for (let index = 0; index < blocks.length; index++) {
    const block = blocks[index];
    if (!block || typeof block !== "object") continue;
    const record = block as Record<string, unknown>;
    const type = textFromUnknown(record.type);
    if (type === "comms") {
      const dedupeKeys = commsNoticeDedupeKeysFromBlock(record, bodyText, index);
      if (consumeCommsNoticeBlockDedupeKeys(dedupeKeys, consumeDuplicateCommsBlock)) {
        consumedDuplicateCommsBlock = true;
        continue;
      }
      // A member-kickoff status (`mob.kickoff_<phase>`, typed lifecycle or
      // the older request form) renders as its typed status, never as a peer
      // message: its content is model-facing routing text.
      const kickoff = memberKickoffNotice(record);
      if (kickoff) {
        const member = decodeMemberAlias(kickoff.member);
        rich.push({
          type: "member-kickoff",
          phase: kickoff.phase,
          member,
          ...(kickoff.role ? { role: kickoff.role } : {}),
          copyText: `Kickoff ${memberKickoffPhaseLabel(kickoff.phase).toLowerCase()}: ${member}`,
        });
        continue;
      }
      const peer = record.peer && typeof record.peer === "object"
        ? record.peer as Record<string, unknown>
        : {};
      const peerLabel = peerLastSegment(textFromUnknown(peer.display_name) || textFromUnknown(peer.id) || "peer");
      const peerAliases = normalizedPeerAliases(
        textFromUnknown(peer.display_name),
        textFromUnknown(peer.id),
      );
      const kind = textFromUnknown(record.kind) || "message";
      const direction = textFromUnknown(record.direction);
      const intent = textFromUnknown(record.intent);
      const requestId = textFromUnknown(record.request_id) || `typed-comms:${peerLabel}:${kind}`;
      const contentBlocks = typedNoticeContentBlocks(record.content, blobBaseUrl);
      const contentText = contentBlocks
        .map((item) => item.type === "paragraph" ? item.text : "")
        .filter(Boolean)
        .join("\n")
        .trim();
      const peerImages = contentBlocks.filter((item) => item.type === "image");
      // A runtime notice's content is model-facing routing text, never an
      // authored message: every member-kickoff request (`mob.kickoff_*`, which
      // carries the full peer transport projection) and every typed lifecycle
      // notice. Its typed summary describes the operation.
      const runtimeNotice = kind === "lifecycle"
        || (kind === "request" && intent.startsWith("mob.kickoff_"));
      const displayBodySource = (runtimeNotice ? "" : contentText) || typedCommsStableBodyText(record) || bodyText;
      const preserveStructuredContentEnvelope = structuredCommsBodyShouldPreserveLeadingEnvelope(
        displayBodySource,
        peerAliases,
      );
      // Typed content is owner text, including envelope-looking authored lines.
      // Preserve it without running the legacy parser or scaffold normalization.
      const ownerContentText = typeof record.content === "string"
        ? record.content
        : Array.isArray(record.content)
          ? record.content.map((part: unknown) => {
            if (typeof part === "string") return part;
            if (!part || typeof part !== "object") return "";
            const textPart = part as Record<string, unknown>;
            if (textPart.type !== "text") return "";
            return typeof textPart.text === "string" ? textPart.text
              : typeof textPart.content === "string" ? textPart.content : "";
          }).join("")
          : "";
      const exactDisplayBody = (!runtimeNotice && ownerContentText)
        || [record.summary, record.body, record.detail].filter((part): part is string => typeof part === "string").join("\n")
        || (typeof body === "string" ? body : "");
      const displayBody = textMode === "markdown" ? exactDisplayBody : normalizeStructuredCommsBodyText(
        displayBodySource,
        preserveStructuredContentEnvelope ? [] : peerAliases,
      );
      rich.push({
        type: "tool-call",
        toolCallId: requestId,
        name: `peer_${kind}`,
        arguments: JSON.stringify(record.payload ?? {}, null, 2),
        status: "success",
        peerIncoming: direction !== "outgoing",
        peerTarget: peerLabel,
        ...(typeof peer.display_name === "string" && peer.display_name.trim()
          ? { peerDisplayLabel: decodeMemberAlias(peerLastSegment(peer.display_name.trim())) } : {}),
        peerIdentity: typeof peer.id === "string" && peer.id ? peer.id : "Unknown peer",
        ...(intent ? { peerIntent: intent } : {}),
        peerBody: displayBody || undefined,
        peerBodyFormat: textMode === "markdown" ? "verbatim" : "legacy",
        ...(peerImages.length > 0 ? { peerImages } : {}),
      });
      continue;
    }
    const legacyDedupeKeys = commsNoticeDedupeKeysFromBlock(record, bodyText, index);
    if (consumeCommsNoticeBlockDedupeKeys(legacyDedupeKeys, consumeDuplicateCommsBlock)) {
      consumedDuplicateCommsBlock = true;
      continue;
    }
    if (type === "external_event") {
      // External events are model-facing delivery envelopes for operator
      // sends. The canonical user-facing render is the user_input frame; if
      // we render these notices too, rich/image sends appear duplicated.
      continue;
    }
    if (type === "tool_config" || type === "mcp") {
      const payload = record.payload && typeof record.payload === "object"
        ? record.payload as Record<string, unknown>
        : record;
      const label = type === "mcp" ? "MCP" : "Tool config";
      const text = bodyText || typedNoticeBlockText(payload) || typedNoticeBlockText(record) || label;
      rich.push({ type: "divider", text });
      continue;
    }
    if (type === "background_job" && typeof record.job_id === "string" && record.job_id.trim()
      && typeof record.status === "string" && record.status.trim()) {
      rich.push({
        type: "background-job", jobId: record.job_id, status: record.status,
        ...(typeof record.display_name === "string" ? { displayName: record.display_name } : {}),
        detail: typeof record.detail === "string" ? record.detail : "",
        copyText: typedNoticeBlockText(record) || bodyText,
      });
      continue;
    }
    if (type === "background_job" || type === "auth" || type === "runtime_notice") {
      const text = typedNoticeBlockText(record) || type.replace(/_/g, " ");
      rich.push({ type: "paragraph", text });
      continue;
    }
    const contentBlocks = typedNoticeContentBlocks(record.content, blobBaseUrl);
    if (contentBlocks.length > 0) {
      rich.push(...contentBlocks);
      continue;
    }
    rich.push({ type: "divider", text: typedNoticeBlockText(record) || "Runtime metadata" });
  }
  if (rich.length === 0 && bodyText && !consumedDuplicateCommsBlock) {
    rich.push({ type: "paragraph", text: bodyText });
  }
  return rich;
}

function historyMessageText(
  message: unknown,
  peerRegistry?: Map<string, string>,
  blobBaseUrl?: string,
  toolResults?: Map<string, HistoryToolResult>,
  sourceKind?: string,
  consumeDuplicateCommsBlock?: (key: string) => boolean,
  textMode: ConversationTextMode = "legacy",
): { role: "user" | "assistant" | "system" | "meta" | null; text: string; blocks?: ConversationRichBlock[] } {
  if (!message || typeof message !== "object") {
    return { role: null, text: "" };
  }
  const record = message as Record<string, unknown>;
  const role = typeof record.role === "string" ? record.role : null;
  switch (role) {
    case "user": {
      const text = extractTextFromContentBlocks(record.content);
      return { role: "user", text };
    }
    case "system_notice": {
      const blocks = typedSystemNoticeBlocksToRich(
        record.blocks,
        record.body,
        blobBaseUrl,
        sourceKind,
        consumeDuplicateCommsBlock,
        textMode,
      );
      const duplicateCommsConsumed = Boolean(
        consumeDuplicateCommsBlock
        && blocks.length === 0
        && systemNoticeBlockRecords(record).some((block, index) => (
          commsNoticeDedupeKeysFromBlock(
            block,
            textFromUnknown(record.body),
            index,
          ).length > 0
        )),
      );
      const text = duplicateCommsConsumed
        ? ""
        : typeof record.body === "string"
        ? record.body
        : blocks.map((block) => block.type === "paragraph" || block.type === "divider" ? block.text : "").filter(Boolean).join("\n");
      return { role: "meta", text, ...(blocks.length > 0 ? { blocks } : {}) };
    }
    case "assistant":
      return { role: "assistant", text: typeof record.content === "string" ? record.content : "" };
    case "block_assistant": {
      const blocks = Array.isArray(record.blocks) ? record.blocks : [];
      const richBlocks = blockAssistantRichBlocks(blocks, peerRegistry, toolResults, textMode, blobBaseUrl);
      const text = blocks
        .map((block) => {
          if (!block || typeof block !== "object") return "";
          const item = block as Record<string, unknown>;
          const blockType = typeof item.block_type === "string"
            ? item.block_type
            : typeof item.type === "string"
              ? item.type
              : "";
          const data = item.data && typeof item.data === "object"
            ? item.data as Record<string, unknown>
            : {};
          if (blockType === "text") {
            if (typeof data.text === "string") return data.text;
            if (typeof item.text === "string") return item.text;
          }
          return "";
        })
        .filter((value) => value.trim().length > 0)
        .join("\n\n");
      return { role: "assistant", text, ...(richBlocks.length > 0 ? { blocks: richBlocks } : {}) };
    }
    case "system":
      return { role: "system", text: typeof record.content === "string" ? record.content : "" };
    default:
      return { role: null, text: "" };
  }
}

function renderSessionHistoryTextCompleteEntry(
  agent: ConsoleAgent | null,
  frame: ConsoleFrame,
  entryId: string,
  options: {
    suppressAssistantText?: boolean;
    consumeDuplicateReasoningBlock?: (text: string) => boolean;
    consumeDuplicateToolBlock?: (block: ConversationRichToolCallBlock) => boolean;
    consumeDuplicateCommsBlock?: (key: string) => boolean;
    peerRegistry?: Map<string, string>;
    blobBaseUrl?: string;
    textMode?: ConversationTextMode;
    toolResults?: Map<string, HistoryToolResult>;
  } = {},
): ConversationTimelineEntry | null {
  if (frame.sourceKind !== "session_history") return null;
  const record = frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown>
    : {};
  const message = record.message && typeof record.message === "object"
    ? record.message as Record<string, unknown> : null;
  const projectedMessage = message?.role === "block_assistant"
    ? { ...message, blocks: Array.isArray(message.blocks) ? message.blocks.filter((block) => {
        if (!block || typeof block !== "object") return true;
        const item = block as Record<string, unknown>;
        const kind = item.block_type || item.type;
        if (kind === "text" && options.suppressAssistantText) return false;
        if (kind === "reasoning" && options.consumeDuplicateReasoningBlock?.(reasoningBlockText(item))) return false;
        return true;
      }) : [] }
    : options.suppressAssistantText && message?.role === "assistant" ? { ...message, content: "" } : record.message;
  const parsed = historyMessageText(
    projectedMessage,
    options.peerRegistry,
    options.blobBaseUrl,
    options.toolResults,
    frame.sourceKind,
    options.consumeDuplicateCommsBlock,
    options.textMode,
  );
  const textMode = options.textMode ?? "legacy";
  const realtimeOrigin = realtimeMessageOrigin(frame);
  const canonicalText = textMode === "markdown" && !options.suppressAssistantText
    ? canonicalHistoryAssistantText(frame) : undefined;
  const text = canonicalText ?? (textMode === "markdown" ? parsed.text : parsed.text.trim());
  const parsedBlocks = Array.isArray(parsed.blocks) ? parsed.blocks : [];
  if (!assistantMessageKey(frame) && !isRealtimeHistoryMessage(frame)) {
    // Legacy rows retain their established reasoning-first presentation.
    parsedBlocks.sort((left, right) => Number(right.type === "thinking") - Number(left.type === "thinking"));
  }
  if (parsed.role === "meta") {
    const filteredParsedBlocks = options.consumeDuplicateToolBlock
      ? parsedBlocks.filter((block) => {
          if (block.type !== "tool-call") return true;
          return !options.consumeDuplicateToolBlock?.(block);
        })
      : parsedBlocks;
    if (!text && filteredParsedBlocks.length === 0) return null;
    const blocks = filteredParsedBlocks.length > 0
      ? filteredParsedBlocks
      : messageTextBlocks(text, textMode);
    return {
      kind: "message",
      id: entryId,
      identity: COMMS_IDENTITY,
      variant: blocks.length > 0 ? "rich" : "meta",
      createdAt: isoFromTimestampMs(frame.timestampMs),
      ...(blocks.length > 0 ? { blocks } : { text }),
    };
  }
  if (parsed.role !== "assistant" || (!text && parsedBlocks.length === 0)) return null;
  if (/^I have acknowledged the addition of the following peers:/i.test(text)) {
    return null;
  }
  const filteredParsedBlocks = options.consumeDuplicateToolBlock
    ? parsedBlocks.filter((block) => {
        if (block.type !== "tool-call") return true;
        return !options.consumeDuplicateToolBlock?.(block);
      })
    : parsedBlocks;
  if (!text && filteredParsedBlocks.length === 0) return null;
  const blocks = filteredParsedBlocks.length > 0
    ? filteredParsedBlocks
    : messageTextBlocks(text, textMode);
  return {
    kind: "message",
    id: entryId,
    identity: agentIdentity(agent),
    ...(realtimeOrigin ? { realtimeOrigin } : {}),
    ...(canonicalText !== undefined ? { copyText: canonicalText } : {}),
    variant: blocks.length > 0 ? "rich" : "plain",
    createdAt: isoFromTimestampMs(frame.timestampMs),
    ...(blocks.length > 0 ? { blocks } : { text }),
  };
}

function renderSystemNoticeEntry(
  frame: ConsoleFrame,
  entryId: string,
  options: {
    consumeDuplicateToolBlock?: (block: ConversationRichToolCallBlock) => boolean;
    consumeDuplicateCommsBlock?: (key: string) => boolean;
    blobBaseUrl?: string;
    textMode?: ConversationTextMode;
  } = {},
): ConversationTimelineEntry | null {
  if (frame.event !== "system_notice") return null;
  const record = frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown>
    : {};
  const rawMessage = record.message && typeof record.message === "object"
    ? record.message as Record<string, unknown>
    : null;
  const message = rawMessage
    ? (textFromUnknown(rawMessage.role) ? rawMessage : { role: "system_notice", ...rawMessage })
    : {
        role: "system_notice",
        kind: record.kind,
        render_class: record.render_class,
        body: record.body,
        blocks: record.blocks,
      };
  if (isExternalEventOnlySystemNotice(message)) return null;
  const parsed = historyMessageText(
    message,
    undefined,
    options.blobBaseUrl,
    undefined,
    frame.sourceKind,
    options.consumeDuplicateCommsBlock,
    options.textMode ?? "legacy",
  );
  if (parsed.role !== "meta") return null;
  const parsedBlocks = Array.isArray(parsed.blocks) ? parsed.blocks : [];
  const filteredParsedBlocks = options.consumeDuplicateToolBlock
    ? parsedBlocks.filter((block) => {
        if (block.type !== "tool-call") return true;
        return !options.consumeDuplicateToolBlock?.(block);
      })
    : parsedBlocks;
  const text = parsed.text.trim();
  if (!text && filteredParsedBlocks.length === 0) return null;
  const blocks = filteredParsedBlocks.length > 0
    ? filteredParsedBlocks
    : parseConversationRichBlocks(text, { displayNormalization: false });
  return {
    kind: "message",
    id: entryId,
    identity: COMMS_IDENTITY,
    variant: blocks.length > 0 ? "rich" : "meta",
    createdAt: isoFromTimestampMs(frame.timestampMs),
    ...(blocks.length > 0 ? { blocks } : { text }),
  };
}

export function mapFramesToTimelineEntries(
  agent: ConsoleAgent | null,
  frames: ConsoleFrame[],
  options: {
    renderInteractionStartsAsUser?: boolean;
    renderTextDeltas?: boolean;
    suppressEmbeddedRunStartedPrompt?: boolean;
    blobBaseUrl?: string;
    textMode?: ConversationTextMode;
    /** The frames are the newest pages of a longer timeline whose older
     * pages are not loaded yet. Session notices older than every loaded
     * frame then wait for those pages instead of rendering above much newer
     * rows. */
    hasOlderHistory?: boolean;
  } = {},
): ConversationTimelineEntry[] {
  // Live streams keep store order so unscoped comms events do not jump
  // into active turns. Persisted interaction history asks for user prompts,
  // so restore the turn-local semantic order before rendering.
  const textMode = options.textMode ?? "legacy";
  const orderedFrames = reconcileRuntimeAppendFrames(reconcileAssistantMessageFrames(reconcileAssistantHistoryPositions(options.renderInteractionStartsAsUser
    ? sortFramesForTranscript(frames)
    : frames)), { unloadedBeforeMs: options.hasOlderHistory ? loadedTimelineStartMs(frames) : undefined });
  const canonicalToolCounterparts = canonicalAssistantToolCounterparts(orderedFrames);
  const realtimeHistoryIds = new Set(orderedFrames.filter(isRealtimeHistoryMessage).map(frame => frame.id));
  const entries: ConversationTimelineEntry[] = [];
  const emittedOperationFeedback = new Set<string>();
  const emittedMcpApps = new Set<string>();
  const toolBlocks = buildToolBlocks(orderedFrames, options.blobBaseUrl);
  const peerRegistry = buildPeerRegistry(orderedFrames);
  const sessionToolResults = historyToolResults(orderedFrames, false, options.blobBaseUrl);
  const occurrenceToolResults = new Map<string, Map<string, HistoryToolResult>>();
  function toolResultsForHistory(frame: ConsoleFrame): Map<string, HistoryToolResult> {
    if (!assistantMessageKey(frame)) return sessionToolResults;
    const scope = JSON.stringify([frame.runtimeKey, frame.identity, frame.sessionId]);
    let result = occurrenceToolResults.get(scope);
    if (!result) {
      const matching = orderedFrames.filter(other => !ownerContextsConflict(frame, other));
      // Current history is authoritative when both views carry a result.
      matching.sort((left, right) => Number(left.sourceKind === "session_history") - Number(right.sourceKind === "session_history"));
      result = historyToolResults(matching, true, options.blobBaseUrl);
      occurrenceToolResults.set(scope, result);
    }
    return result;
  }

  const structuredCommsSignatures = structuredCommsNoticeTextSignatures(orderedFrames);
  const structuredCommsPromptSuppression = structuredCommsPromptSuppressionKeys(
    orderedFrames,
    structuredCommsSignatures,
  );
  const emittedToolCalls = new Set<string>();
  const {
    liveToolCallIds,
    liveToolSignatureCounts,
  } = liveToolDedupeState(orderedFrames, toolBlocks);
  const assistantHistory = buildAssistantHistoryReconciliation(orderedFrames, options.renderTextDeltas !== false);
  const emittedImages = new Set<string>();
  const emittedUserInputs = new Set<string>();
  // First-emitted user entry per dedupe key. A later twin of the same input
  // (live send frame vs persisted history vs run_started prompt) may be the
  // only one carrying the typed origin; fold it onto the kept entry.
  const emittedUserEntries = new Map<string, ConversationTimelineEntry>();
  const foldUserOrigin = (userKey: string, twin: ConversationTimelineEntry) => {
    const kept = emittedUserEntries.get(userKey);
    if (!kept || kept.kind !== "message" || twin.kind !== "message" || !twin.origin) return;
    kept.origin = { ...twin.origin, ...(kept.origin ?? {}) };
  };
  const emittedCommsNotices = new Map<string, { sourceKind?: string; timestampMs?: number }>();

  let pendingText = "";
  let pendingId = "";
  let pendingCreatedAt: string | undefined;
  type OpenReasoning = {
    block: { type: "thinking"; label: string; text: string; final?: boolean };
    scoped: boolean;
    scope: string;
  };
  // ReasoningComplete closes the assembler's current block. A visual flush
  // leaves that block available for a late completion; completed blocks never
  // participate in content-based matching with a later block.
  const openReasoning = new Map<string, OpenReasoning>();
  let activeReasoning: OpenReasoning | undefined;
  let streamedOwner: AssistantFrameOwner | undefined;
  // A visible input can interrupt one run while that run keeps streaming.
  // Keep completion evidence per owner; flushing a row changes presentation,
  // not the source document whose terminal frame will arrive later.
  const streamedTextByOwner = new Map<string, { owner: AssistantFrameOwner; text: string }[]>();
  function ownedStream(frame: AssistantFrameOwner) {
    return streamedTextByOwner.get(assistantOwnerKey(frame))?.find((stream) => sameTextStreamOwner(stream.owner, frame));
  }
  function appendOwnedStream(frame: AssistantFrameOwner, delta: string) {
    const existing = ownedStream(frame);
    if (existing) {
      existing.text += delta;
      return;
    }
    const key = assistantOwnerKey(frame);
    const streams = streamedTextByOwner.get(key) || [];
    streams.push({ owner: frame, text: delta });
    streamedTextByOwner.set(key, streams);
  }
  function forgetOwnedStream(frame: AssistantFrameOwner) {
    const key = assistantOwnerKey(frame);
    const streams = streamedTextByOwner.get(key)?.filter((stream) => !sameTextStreamOwner(stream.owner, frame)) || [];
    if (streams.length) streamedTextByOwner.set(key, streams);
    else streamedTextByOwner.delete(key);
  }
  const completedTextByOwner = new Map<string, { owner: AssistantFrameOwner; text: string }[]>();
  function completedStream(frame: AssistantFrameOwner) {
    return completedTextByOwner.get(assistantOwnerKey(frame))?.find((stream) => sameTextStreamOwner(stream.owner, frame));
  }
  function forgetCompletedStream(frame: AssistantFrameOwner) {
    const key = assistantOwnerKey(frame);
    const streams = completedTextByOwner.get(key)?.filter((stream) => !sameTextStreamOwner(stream.owner, frame)) || [];
    if (streams.length) completedTextByOwner.set(key, streams);
    else completedTextByOwner.delete(key);
  }
  function completeOwnedStream(frame: AssistantFrameOwner, text = ownedStream(frame)?.text || "") {
    if (text) {
      forgetCompletedStream(frame);
      const key = assistantOwnerKey(frame);
      const streams = completedTextByOwner.get(key) || [];
      streams.push({ owner: frame, text });
      completedTextByOwner.set(key, streams);
    }
    forgetOwnedStream(frame);
  }
  function forgetUnscopedStream() {
    if (streamedOwner && !streamedOwner.runId?.trim() && !streamedOwner.interactionId?.trim()) {
      forgetOwnedStream(streamedOwner);
      forgetCompletedStream(streamedOwner);
    }
  }

  function reasoningScope(frame: ConsoleFrame): { scope: string; scoped: boolean } {
    const messageKey = assistantMessageKey(frame);
    if (messageKey) return { scoped: true, scope: JSON.stringify([frame.runtimeKey || "", frame.identity || "", messageKey]) };
    if (hasAssistantMessageIdCarrier(frame)) return { scoped: false, scope: `unbound:${frame.id}` };
    const interactionId = frame.interactionId?.trim() || "";
    const runId = frame.runId?.trim() || "";
    const turnId = frame.turnId?.trim() || "";
    return {
      scoped: Boolean(interactionId || runId || turnId),
      scope: JSON.stringify([
        frame.runtimeKey || "", frame.identity || "", frame.sessionId || "",
        interactionId, runId, turnId, frame.sourceKind || "console_event",
      ]),
    };
  }

  function startReasoning(frame: ConsoleFrame, entryId: string): OpenReasoning {
    const state: OpenReasoning = {
      ...reasoningScope(frame),
      block: { type: "thinking", label: "", text: "" },
    };
    const interactionId = frame.interactionId?.trim();
    const createdAt = isoFromTimestampMs(frame.timestampMs);
    entries.push({
      kind: "message",
      id: entryId,
      identity: agentIdentity(agent),
      variant: "rich",
      // The opening source frame owns the entry identity through completion.
      ...(interactionId ? { interactionId } : {}),
      ...(frame.runId ? { runId: frame.runId } : {}),
      ...(createdAt ? { createdAt } : {}),
      blocks: [state.block],
    });
    openReasoning.set(state.scope, state);
    return state;
  }

  function flushPendingReasoning(final = false) {
    if (!activeReasoning) return;
    if (final) activeReasoning.block.final = true;
    // Without a typed turn/run/interaction, a later event after a visual
    // boundary cannot be claimed as the completion of this block.
    if (!activeReasoning.scoped) openReasoning.delete(activeReasoning.scope);
    activeReasoning = undefined;
  }

  function flushPendingText(final = true) {
    if (!pendingText) return;
    const blocks = messageTextBlocks(pendingText, textMode, !final);
    entries.push({
      kind: "message",
      id: pendingId,
      identity: agentIdentity(agent),
      variant: blocks.length > 0 ? "rich" : "plain",
      // Reconciliation identity from the first frame (see flushPendingReasoning).
      ...(streamedOwner?.interactionId ? { interactionId: streamedOwner.interactionId } : {}),
      ...(streamedOwner?.runId ? { runId: streamedOwner.runId } : {}),
      ...(pendingCreatedAt ? { createdAt: pendingCreatedAt } : {}),
      ...(blocks.length > 0 ? { blocks } : { text: pendingText }),
    });
    pendingText = "";
    pendingId = "";
    pendingCreatedAt = undefined;
  }

  for (let i = 0; i < orderedFrames.length; i++) {
    const frame = orderedFrames[i];
    // Canonical frame identity must survive insertion of older history.
    const entryId = frame.id || `${frame.event || "frame"}:${i}`;

    if (frame.event === "mcp_app") {
      const entry = mcpAppEntryFromFrame(frame, entryId, agentIdentity(agent));
      if (!entry || emittedMcpApps.has(entry.renderKey)) continue;
      emittedMcpApps.add(entry.renderKey);
      flushPendingReasoning(true);
      flushPendingText();
      entries.push(entry);
      continue;
    }

    // These events bound the assistant response's tool/turn lifecycle.
    // They also close a message when TextComplete was omitted; user/steer inputs,
    // reasoning, server tools and image blocks are not message boundaries.
    if (frame.runId?.trim() && frame.sourceKind !== "session_history" && (
      frame.event === "tool_call_requested" || frame.event === "tool_call"
      || frame.event === "tool_execution_started" || frame.event === "tool_result_received"
      || frame.event === "tool_execution_completed" || frame.event === "tool_execution_timed_out"
      || frame.event === "turn_completed" || frame.event === "turn_started"
    )) {
      if (sameTextStreamOwner(streamedOwner, frame)) flushPendingText();
      completeOwnedStream(frame);
    }

    const operationFeedback = operationFeedbackFromFrame(frame);
    if (operationFeedback) {
      flushPendingReasoning(true);
      flushPendingText();
      const key = JSON.stringify([frame.runtimeKey, frame.identity, frame.sessionId,
        frame.runId, frame.interactionId, operationFeedback.kind,
        operationFeedback.operationId ?? operationFeedback.toolCallId ?? parseToolCallId(frame) ?? entryId]);
      if (!emittedOperationFeedback.has(key)) {
        emittedOperationFeedback.add(key);
        entries.push({ kind: "message", id: entryId, identity: SYSTEM_IDENTITY,
          variant: "meta", createdAt: isoFromTimestampMs(frame.timestampMs),
          runId: frame.runId, interactionId: frame.interactionId,
          text: operationFeedback.detail, operationFeedback });
      }
      continue;
    }

    if (frame.event === "reasoning_delta") {
      const delta = reasoningFrameText(frame);
      if (!delta) continue;
      const { scope } = reasoningScope(frame);
      if (activeReasoning?.scope !== scope) flushPendingReasoning(true);
      flushPendingText();
      const state = openReasoning.get(scope) || startReasoning(frame, entryId);
      state.block.text += delta;
      delete state.block.final;
      activeReasoning = state;
      continue;
    }

    if (frame.event === "reasoning_complete") {
      const { scope } = reasoningScope(frame);
      const unfinished = openReasoning.get(scope);
      if (activeReasoning !== unfinished) flushPendingReasoning(true);
      flushPendingText();
      const text = reasoningFrameText(frame);
      if (unfinished || text.trim()) {
        const state = unfinished || startReasoning(frame, entryId);
        state.block.text = text;
        state.block.final = true;
        openReasoning.delete(scope);
        if (activeReasoning === state) activeReasoning = undefined;
      }
      continue;
    }

    if (frame.event === "text_delta") {
      if (options.renderTextDeltas === false) {
        continue;
      }
      flushPendingReasoning(true);
      if (!sameTextStreamOwner(streamedOwner, frame)) {
        flushPendingText();
        forgetUnscopedStream();
        streamedOwner = frame;
      }
      const delta = assistantHistory.deltaOverrides.get(frame.id) ?? summarizeFrameData(frame.data);
      if (!delta) continue;
      if (!pendingId) {
        pendingId = entryId;
        pendingCreatedAt = isoFromTimestampMs(frame.timestampMs);
      }
      pendingText += delta;
      forgetCompletedStream(frame);
      appendOwnedStream(frame, delta);
      continue;
    }

    if (frame.event === "assistant_image" || frame.event === "assistant_image_appended") {
      flushPendingReasoning(true);
      flushPendingText();
      const imageEntry = renderAssistantImageEntry(agent, frame, entryId, options.blobBaseUrl);
      if (imageEntry) {
        const messageKey = assistantMessageKey(frame);
        const imageKey = imageEntryKey(imageEntry, Boolean(messageKey));
        const key = imageKey && messageKey ? JSON.stringify([frame.runtimeKey, frame.identity, messageKey, imageKey]) : imageKey;
        if (key && emittedImages.has(key)) {
          continue;
        }
        if (key) emittedImages.add(key);
        entries.push(imageEntry);
      }
      continue;
    }

    const toolCallId = parseToolCallId(frame);
    if (canonicalToolCounterparts.has(frame)) continue;
    if (
      toolCallId
      && (
        frame.event === "tool_call_requested" ||
        frame.event === "tool_call" ||
        frame.event === "tool_execution_started" ||
        frame.event === "server_tool_content"
      )
      && !emittedToolCalls.has(toolCallId)
    ) {
      flushPendingReasoning(true);
      flushPendingText();
      const block = toolBlocks.get(toolCallId);
      if (block) {
        // Group consecutive tool calls of the same `name` into one
        // rich entry. Any frame that produces its own visible entry
        // (text bubble, user message, system notice) lands between
        // the tool entries in `entries[]` and breaks this match — so
        // grouping naturally fires only for runs of same-tool calls
        // with no user-facing output between them. Peer tools have
        // an extra direction constraint (incoming vs outgoing) so a
        // fresh `send_response` doesn't fold into a `send_request`
        // group.
        const lastEntry = entries[entries.length - 1];
        const lastBlocks = lastEntry
          && lastEntry.kind === "message"
          && lastEntry.variant === "rich"
          && Array.isArray(lastEntry.blocks)
          ? lastEntry.blocks as unknown as Array<Record<string, unknown>>
          : null;
        const lastIsToolGroup = !!(lastBlocks
          && lastBlocks.length > 0
          && lastBlocks.every((b) => b.type === "tool-call"));
        const lastSameName = lastIsToolGroup
          && lastBlocks!.every((b) => b.name === block.name);
        const newIncoming = block.peerIncoming === true;
        const peerCompatible = !block.peerTarget
          || (lastIsToolGroup
              && lastBlocks!.every((b) => Boolean(b.peerIncoming) === newIncoming));

        if (lastSameName && peerCompatible) {
          ((lastEntry as unknown as { blocks: unknown[] }).blocks).push(block);
        } else {
          entries.push({
            kind: "message",
            id: entryId,
            identity: agentIdentity(agent),
            variant: "rich",
            createdAt: isoFromTimestampMs(frame.timestampMs),
            blocks: [block],
          });
        }
        emittedToolCalls.add(toolCallId);
      }
      continue;
    }

    if (frame.event === "tool_result_received" || frame.event === "tool_execution_completed" || frame.event === "tool_execution_timed_out") {
      flushPendingReasoning(true);
      const imageEntries = renderGeneratedImageToolResultEntries(
        agent,
        frame,
        entryId,
        options.blobBaseUrl,
      );
      for (const imageEntry of imageEntries) {
        const messageKey = assistantMessageKey(frame);
        const imageKey = imageEntryKey(imageEntry, Boolean(messageKey));
        const key = imageKey && messageKey ? JSON.stringify([frame.runtimeKey, frame.identity, messageKey, imageKey]) : imageKey;
        if (key && emittedImages.has(key)) continue;
        if (key) emittedImages.add(key);
        entries.push(imageEntry);
      }
      continue;
    }

    if (options.renderInteractionStartsAsUser && (frame.event === "interaction_started" || frame.event === "user_input")) {
      flushPendingReasoning(true);
      flushPendingText();
      if (!sameTextStreamOwner(streamedOwner, frame)) {
        forgetUnscopedStream();
        streamedOwner = frame;
      }
      const userEntry = renderHistoryUserEntry(frame, entryId, options.blobBaseUrl, textMode);
      if (userEntry) {
        const userKey = userEntryDedupeKey(frame, userEntry);
        if (userKey && emittedUserInputs.has(userKey)) {
          foldUserOrigin(userKey, userEntry);
        } else {
          if (userKey) {
            emittedUserInputs.add(userKey);
            emittedUserEntries.set(userKey, userEntry);
          }
          entries.push(userEntry);
        }
      }
      const failureEntry = renderUserDeliveryFailureEntry(frame, entryId);
      if (failureEntry) entries.push(failureEntry);
      continue;
    }

    if (frame.event === "run_started") {
      flushPendingReasoning(true);
      flushPendingText();
      const promptEntries = renderRunStartedPromptEntries(frame, entryId, {
        textMode,
        suppressEmbeddedRpcPrompt: options.suppressEmbeddedRunStartedPrompt === true,
        suppressStructuredCommsPrompt: structuredCommsPromptSuppression.has(entryId),
        blobBaseUrl: options.blobBaseUrl,
      });
      if (promptEntries.length > 0) {
        for (const promptEntry of promptEntries) {
          const userKey = userPromptDedupeKey(frame, promptEntry);
          if (userKey && emittedUserInputs.has(userKey)) {
            foldUserOrigin(userKey, promptEntry);
            continue;
          }
          if (userKey) {
            emittedUserInputs.add(userKey);
            emittedUserEntries.set(userKey, promptEntry);
          }
          entries.push(promptEntry);
        }
        continue;
      }
    }

    if (frame.event === "system_notice") {
      flushPendingReasoning(true);
      flushPendingText();
      if (shouldSuppressDuplicateCommsNotice(frame, emittedCommsNotices)) {
        continue;
      }
      const noticeEntry = renderSystemNoticeEntry(frame, entryId, {
        blobBaseUrl: options.blobBaseUrl,
        textMode,
        consumeDuplicateCommsBlock: (key) => {
          const runtimeKey = runtimeAppendNoticeKey(frame);
          if (runtimeKey) key = `${runtimeKey}:${key}`;
          if (commsNoticeDuplicateKey(key, frame, emittedCommsNotices)) {
            return true;
          }
          markCommsNoticeDedupeKey(key, frame, emittedCommsNotices);
          return false;
        },
        consumeDuplicateToolBlock: (block) => (
          liveToolCallIds.has(block.toolCallId)
          || consumeToolSignatureCount(liveToolSignatureCounts, block)
        ),
      });
      if (noticeEntry) {
        entries.push(noticeEntry);
      }
      continue;
    }

    if (frame.sourceKind === "session_history" && (
      frame.event === "assistant_message" || frame.event === "text_complete" || frame.event === "interaction_complete"
      || frame.event === "run_completed" || frame.event === "interaction_failed" || frame.event === "run_failed"
    )) {
      const suppressAssistantText = assistantHistory.consumedHistory.has(frame.id);
      if (suppressAssistantText && !historyHasAssistantSiblings(frame)) continue;
      const historyEntry = renderSessionHistoryTextCompleteEntry(agent, frame, entryId, {
        suppressAssistantText,
        consumeDuplicateReasoningBlock: (text) => assistantHistory.consumeReasoning(frame, text),
        textMode,
        peerRegistry,
        blobBaseUrl: options.blobBaseUrl,
        toolResults: toolResultsForHistory(frame),
        consumeDuplicateCommsBlock: (key) => {
          const runtimeKey = runtimeAppendNoticeKey(frame);
          if (runtimeKey) key = `${runtimeKey}:${key}`;
          if (commsNoticeDuplicateKey(key, frame, emittedCommsNotices)) {
            return true;
          }
          markCommsNoticeDedupeKey(key, frame, emittedCommsNotices);
          return false;
        },
        consumeDuplicateToolBlock: (block) => (
          assistantMessageKey(frame) || isRealtimeHistoryMessage(frame)
            ? false
            : liveToolCallIds.has(block.toolCallId)
              || consumeToolSignatureCount(liveToolSignatureCounts, block)
        ),
      });
      if (historyEntry) {
        historyEntry.interactionId = frame.interactionId?.trim() || undefined;
        if (historyEntry.kind === "message") historyEntry.runId = frame.runId?.trim() || undefined;
        if (!hasAssistantMessageIdCarrier(frame) && shouldSuppressRepeatedAssistantEntry(historyEntry, entries, realtimeHistoryIds)) {
          continue;
        }
        if (conversationEntryVisibleText(historyEntry)) flushPendingText();
        entries.push(historyEntry);
      }
      continue;
    }

    if (!frame.runId?.trim() && frame.event === "text_complete") {
      flushPendingReasoning(true);
      if (frame.sourceKind !== "session_history") {
        const text = terminalFrameVisibleText(frame).trim();
        if (
          text
          && pendingText
          && sameTextStreamOwner(streamedOwner, frame)
          && normalizeComparableText(pendingText) === normalizeComparableText(text)
        ) {
          continue;
        }
        const duplicateTerminalFollows = text
          && orderedFrames.slice(i + 1).some((later) => {
            if (
              later.event !== "interaction_complete"
              && later.event !== "run_completed"
            ) {
              return false;
            }
            if (later.sourceKind === "session_history" || !sameTextStreamOwner(frame, later)) {
              return false;
            }
            return normalizeComparableText(terminalFrameVisibleText(later)) === normalizeComparableText(text);
          });
        if (duplicateTerminalFollows) {
          continue;
        }
      }
    }

    if (
      frame.event === "interaction_complete"
      || frame.event === "run_completed"
      || frame.event === "text_complete"
      || frame.event === "interaction_failed"
      || frame.event === "run_failed"
    ) {
      // Replayed completion can arrive before the final live chunk. Ignored
      // history must not flush or reset that still-streaming document.
      const ownsStream = sameTextStreamOwner(streamedOwner, frame);
      const streamedText = ownedStream(frame)?.text
        || (frame.event !== "text_complete" ? completedStream(frame)?.text : "")
        || (ownsStream ? pendingText : "");
      flushPendingReasoning(true);
      flushPendingText();
      // RunCompleted.result is the last assistant message. Keep its typed
      // completion evidence until another message starts for this owner.
      if (frame.runId?.trim()) completeOwnedStream(frame, terminalFrameVisibleText(frame));
      else forgetOwnedStream(frame);
      if (ownsStream) streamedOwner = undefined;
      const terminalEntry = renderTerminalEntry(agent, frame, entryId, streamedText, textMode);
      if (terminalEntry) {
        terminalEntry.interactionId = frame.interactionId?.trim() || undefined;
        if (terminalEntry.kind === "message") terminalEntry.runId = frame.runId?.trim() || undefined;
        if (!hasAssistantMessageIdCarrier(frame) && shouldSuppressRepeatedAssistantEntry(terminalEntry, entries, realtimeHistoryIds)) {
          continue;
        }
        entries.push(terminalEntry);
      }
      continue;
    }

    if (HIDDEN_EVENTS.has(frame.event)) {
      continue;
    }

    flushPendingReasoning(true);
    flushPendingText();

    // Try to render as a clean peer message
    const peerEntry = renderPeerEntry(frame, entryId);
    if (peerEntry) {
      entries.push(peerEntry);
      continue;
    }

    // Skip remaining tool lifecycle events (handled by tool blocks above)
    if (frame.event === "tool_call_requested" || frame.event === "tool_call" || frame.event === "tool_execution_started"
      || frame.event === "tool_result_received" || frame.event === "tool_execution_completed" || frame.event === "tool_execution_timed_out") {
      continue;
    }

    // Memory subsystem events render as clean meta lines — never raw JSON.
    if (frame.event.startsWith("memory.")) {
      entries.push({
        kind: "message",
        id: entryId,
        identity: SYSTEM_IDENTITY,
        variant: "meta",
        createdAt: isoFromTimestampMs(frame.timestampMs),
        text: describeMemoryTimelineEvent(
          frame.event,
          frame.data && typeof frame.data === "object"
            ? (frame.data as Record<string, unknown>)
            : {},
        ),
      });
      continue;
    }

    // Any other runtime event: typed record plus a plain-language line. The
    // raw payload rides on the entry for an expandable details view and is
    // never printed inline.
    const runtimeEvent = runtimeEventFromFrame(frame.event, frame.data);
    entries.push({
      kind: "message",
      id: entryId,
      identity: SYSTEM_IDENTITY,
      variant: "meta",
      createdAt: isoFromTimestampMs(frame.timestampMs),
      text: runtimeEventText(runtimeEvent),
      runtimeEvent,
    });
  }

  flushPendingReasoning(false);
  flushPendingText(false);
  const renderKeys = new Map(orderedFrames.filter(frame => frame.event === "text_delta"
    || frame.event === "text_complete" || frame.event === "assistant_message"
    || frame.event === "reasoning_delta" || frame.event === "reasoning_complete")
    .map(frame => [frame.id, assistantMessageRenderKey(frame)]));
  const userRenderKeys = new Map(orderedFrames.map(frame => [frame.id, userMessageRenderKey(frame)]));
  return assistantPresentationEntries(entries.filter((entry) => entry.kind !== "message"
    || entry.blocks?.length !== 1
    || entry.blocks[0].type !== "thinking"
    || entry.blocks[0].text.trim()).map((entry) => {
    if (entry.kind !== "message") return entry;
    if (entry.identity.role === "user") {
      const userKey = userRenderKeys.get(entry.id);
      if (userKey) entry = { ...entry, renderKey: userKey };
    }
    if (!entry.blocks?.some((block) => block.type === "markdown")) return entry;
    let textIndex = 0;
    return { ...entry, blocks: entry.blocks.map((block) => block.type === "markdown"
      ? { ...block, id: `${entry.renderKey ?? entry.id}:text:${textIndex++}` }
      : block) };
  }), renderKeys, new Map(orderedFrames.map(frame => [frame.id, frame])));
}

export function createUserEntry(
  message: string,
  images: Array<{ src: string; mediaType: string; alt?: string }> = [],
  options: { textMode?: ConversationTextMode } = {},
): ConversationTimelineEntry {
  const textMode = options.textMode ?? "legacy";
  if (images.length > 0 || textMode === "markdown") {
    const blocks: ConversationRichBlock[] = [
      ...messageTextBlocks(message, textMode),
      ...images.map((image) => ({
        type: "image" as const,
        src: image.src,
        mediaType: image.mediaType,
        alt: image.alt || "attached image",
      })),
    ];
    return {
      kind: "message",
      id: `user:${Date.now()}`,
      identity: USER_IDENTITY,
      variant: "rich",
      createdAt: new Date().toISOString(),
      blocks,
    };
  }
  return {
    kind: "message",
    id: `user:${Date.now()}`,
    identity: USER_IDENTITY,
    variant: "plain",
    createdAt: new Date().toISOString(),
    text: message,
  };
}

export function sortConversationTimelineEntries(
  entries: ConversationTimelineEntry[],
): ConversationTimelineEntry[] {
  return entries
    .map((entry, index) => ({ entry, index }))
    .sort((left, right) => {
      const leftTs = Date.parse(String(left.entry.createdAt || ""));
      const rightTs = Date.parse(String(right.entry.createdAt || ""));
      const safeLeft = Number.isFinite(leftTs) ? leftTs : Number.NaN;
      const safeRight = Number.isFinite(rightTs) ? rightTs : Number.NaN;
      if (Number.isFinite(safeLeft) && Number.isFinite(safeRight) && safeLeft !== safeRight) {
        return safeLeft - safeRight;
      }
      if (Number.isFinite(safeLeft) && !Number.isFinite(safeRight)) {
        return 1;
      }
      if (!Number.isFinite(safeLeft) && Number.isFinite(safeRight)) {
        return -1;
      }
      return left.index - right.index;
    })
    .map(({ entry }) => entry);
}

export function appendOptimisticConversationEntry(
  entries: ConversationTimelineEntry[],
  optimisticEntry: ConversationTimelineEntry | null | undefined,
): ConversationTimelineEntry[] {
  return optimisticEntry ? [...entries, optimisticEntry] : entries;
}

function isIntermediateHistoryAssistantStep(frame: ConsoleFrame): boolean {
  if (frame.sourceKind !== "session_history"
    || (frame.event !== "text_complete" && frame.event !== "interaction_complete")) return false;
  const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
  const message = data.message && typeof data.message === "object" ? data.message as Record<string, unknown> : {};
  return message.role === "block_assistant" && message.stop_reason === "tool_use";
}

type LifecyclePhase = { phase: ResponsePhase; order: number };

type LifecycleOwner = {
  context: Pick<ConsoleFrame, "runtimeKey" | "identity" | "sessionId">;
  interactionId?: string;
  runIds: Set<string>;
  openRuns: Set<string>;
  runStartOrder: Map<string, number>;
  runPhases: Map<string, LifecyclePhase>;
  interactionOpen: boolean;
  unidentifiedRunOpen: boolean;
  inputOpen: boolean;
  auxiliaryOpen: boolean;
  ownerPhase: LifecyclePhase;
  phase: ResponsePhase;
  order: number;
};

const HISTORICAL_AUXILIARY_EVENTS = new Set([
  "reasoning_delta", "reasoning_complete", "tool_call_requested", "tool_call",
  "tool_execution_started", "tool_result_received", "tool_execution_completed", "server_tool_content",
]);

// Display projection of supplied lifecycle IDs, not execution authority.
export class ConsoleActivityProjection {
  private interactions = new Map<string, LifecycleOwner[]>();
  private runs = new Map<string, LifecycleOwner[]>();
  private anonymous: LifecycleOwner[] = [];
  private active = new Set<LifecycleOwner>();
  private busyOwners = new Set<LifecycleOwner>();
  private interactionsOpen = new Set<LifecycleOwner>();
  private runsOpen = new Set<LifecycleOwner>();
  private phaseOwner?: LifecycleOwner;
  private order = 0;
  phase: ResponsePhase;
  terminal = false;

  constructor(fallback: ResponsePhase = null) { this.phase = fallback; }
  get busy(): boolean { return this.busyOwners.size > 0; }
  get interactionOpen(): boolean { return this.interactionsOpen.size > 0; }
  get runOpen(): boolean { return this.runsOpen.size > 0; }

  private matching(owners: LifecycleOwner[] | undefined, frame: ConsoleFrame): LifecycleOwner | undefined {
    const matches = owners?.filter(owner => !ownerContextsConflict(owner.context, frame)
      && !(owner.interactionId && frame.interactionId && owner.interactionId !== frame.interactionId));
    return matches?.length === 1 ? matches[0] : undefined;
  }
  private index(map: Map<string, LifecycleOwner[]>, key: string, owner: LifecycleOwner): void {
    const owners = map.get(key) ?? [];
    if (!owners.includes(owner)) map.set(key, [...owners, owner]);
  }
  private forget(owner: LifecycleOwner): void {
    const remove = (map: Map<string, LifecycleOwner[]>, key: string) => {
      const remaining = map.get(key)?.filter(value => value !== owner) ?? [];
      if (remaining.length) map.set(key, remaining); else map.delete(key);
    };
    if (owner.interactionId) remove(this.interactions, owner.interactionId);
    for (const id of owner.runIds) remove(this.runs, id);
    this.anonymous = this.anonymous.filter(value => value !== owner);
    this.active.delete(owner);
    this.busyOwners.delete(owner);
    this.interactionsOpen.delete(owner);
    this.runsOpen.delete(owner);
  }
  private owner(frame: ConsoleFrame): LifecycleOwner | undefined {
    const interaction = typeof frame.interactionId === "string" && frame.interactionId.length ? frame.interactionId : undefined;
    const run = typeof frame.runId === "string" && frame.runId.length ? frame.runId : undefined;
    const byInteraction = interaction ? this.matching(this.interactions.get(interaction), frame) : undefined;
    const byRun = run ? this.matching(this.runs.get(run), frame) : undefined;
    if (byInteraction && byRun && ownerContextsConflict(byInteraction.context, byRun.context)) return undefined;
    let owner = byInteraction ?? byRun;
    // A frame carrying both IDs establishes their relationship. An earlier
    // run-only observation may then join the separately observed interaction.
    if (byInteraction && byRun && byInteraction !== byRun && !byRun.interactionId) {
      for (const key of ["runtimeKey", "identity", "sessionId"] as const) byInteraction.context[key] ??= byRun.context[key];
      for (const id of byRun.runIds) { byInteraction.runIds.add(id); this.index(this.runs, id, byInteraction); }
      for (const id of byRun.openRuns) byInteraction.openRuns.add(id);
      for (const [id, order] of byRun.runStartOrder) {
        if (order < (byInteraction.runStartOrder.get(id) ?? Infinity)) byInteraction.runStartOrder.set(id, order);
      }
      for (const [id, phase] of byRun.runPhases) {
        if (phase.order > (byInteraction.runPhases.get(id)?.order ?? -1)) byInteraction.runPhases.set(id, phase);
      }
      byInteraction.interactionOpen ||= byRun.interactionOpen;
      byInteraction.unidentifiedRunOpen ||= byRun.unidentifiedRunOpen;
      byInteraction.inputOpen ||= byRun.inputOpen;
      byInteraction.auxiliaryOpen ||= byRun.auxiliaryOpen;
      if (byRun.ownerPhase.order > byInteraction.ownerPhase.order) byInteraction.ownerPhase = byRun.ownerPhase;
      this.selectOwnerPhase(byInteraction);
      this.forget(byRun);
      owner = byInteraction;
    }
    if (!interaction && !run) owner = this.matching(this.anonymous, frame);
    if (!owner) {
      owner = { context: { runtimeKey: frame.runtimeKey, identity: frame.identity, sessionId: frame.sessionId },
        runIds: new Set(), openRuns: new Set(), runStartOrder: new Map(), runPhases: new Map(), interactionOpen: false, unidentifiedRunOpen: false,
        inputOpen: false, auxiliaryOpen: false, ownerPhase: { phase: null, order: 0 }, phase: null, order: 0 };
      if (!interaction && !run) this.anonymous.push(owner);
    }
    for (const key of ["runtimeKey", "identity", "sessionId"] as const) owner.context[key] ??= frame[key];
    if (interaction) { owner.interactionId = interaction; this.index(this.interactions, interaction, owner); }
    if (run) { owner.runIds.add(run); this.index(this.runs, run, owner); }
    return owner;
  }
  private selectOwnerPhase(owner: LifecycleOwner): void {
    const previousOrder = owner.order;
    let runPhase: LifecyclePhase | undefined;
    let clearedRunOrder = 0;
    for (const contribution of owner.runPhases.values()) {
      if (contribution.phase === null) clearedRunOrder = Math.max(clearedRunOrder, contribution.order);
      else if ((owner.ownerPhase.phase !== null || contribution.order > owner.ownerPhase.order)
        && (!runPhase || contribution.order > runPhase.order)) runPhase = contribution;
    }
    // A run-local clear invalidates an older owner fallback, while another
    // run's live contribution survives. An owner-wide clear covers older runs.
    let latest = clearedRunOrder > owner.ownerPhase.order ? { phase: null, order: clearedRunOrder } : owner.ownerPhase;
    if (runPhase && (latest.phase === null || runPhase.order > latest.order)) latest = runPhase;
    owner.phase = latest.phase;
    owner.order = latest.order;
    // A settled run can reveal an older phase. Let another owner's newer
    // contribution win rather than dating the survivor with the terminal.
    if (this.phaseOwner === owner && owner.order < previousOrder) this.selectPhaseOwner();
  }
  private selectPhaseOwner(): void {
    this.phaseOwner = undefined;
    for (const active of this.active) {
      if (active.phase !== null && (!this.phaseOwner || active.order > this.phaseOwner.order)) this.phaseOwner = active;
    }
  }
  private refreshOwner(owner: LifecycleOwner, retainAssociation = false): void {
    const hasRun = owner.unidentifiedRunOpen || owner.openRuns.size > 0;
    const open = owner.interactionOpen || hasRun || owner.inputOpen || owner.auxiliaryOpen;
    if (retainAssociation && !open && owner.phase === null) return;
    if (owner.interactionOpen) this.interactionsOpen.add(owner); else this.interactionsOpen.delete(owner);
    if (hasRun) this.runsOpen.add(owner); else this.runsOpen.delete(owner);
    if (open) this.busyOwners.add(owner); else this.busyOwners.delete(owner);
    if (open || owner.phase !== null) {
      this.active.add(owner);
      if (owner.phase !== null && (!this.phaseOwner || owner.order > this.phaseOwner.order)) this.phaseOwner = owner;
    } else this.forget(owner);
    // Visual text completion need not settle queue-busy ownership. Only a
    // settled displayed phase scans other active contributions, never tokens.
    if (!this.phaseOwner || !this.active.has(this.phaseOwner) || this.phaseOwner.phase === null) {
      this.selectPhaseOwner();
    }
    this.phase = this.phaseOwner?.phase ?? null;
  }
  fold(frame: ConsoleFrame): boolean {
    // Saved tool and reasoning rows describe transcript content. Their append
    // cursors, message IDs and arrival order do not reserve current work.
    // Retain exact ID associations in the existing indexes, even before live
    // activity arrives. An inactive association opens no lifecycle or phase.
    if (frame.sourceKind === "session_history" && HISTORICAL_AUXILIARY_EVENTS.has(frame.event)) {
      if (typeof frame.runId !== "string" || !frame.runId
        || typeof frame.interactionId !== "string" || !frame.interactionId) return false;
      if (this.runs.get(frame.runId)?.some(owner => !ownerContextsConflict(owner.context, frame)
        && owner.interactionId && owner.interactionId !== frame.interactionId)) return false;
      const owner = this.owner(frame);
      if (!owner) return false;
      this.refreshOwner(owner, true);
      this.terminal &&= this.phase === null;
      return true;
    }
    if (isIntermediateHistoryAssistantStep(frame)) return false;
    const terminalUser = frame.event === "user_input" && isTerminalUserInputStatus(frame.status);
    let phase: ResponsePhase | undefined;
    let terminal = false;
    switch (frame.event) {
      case "user_input": phase = terminalUser ? null : "waiting"; terminal = terminalUser; break;
      case "interaction_started": case "run_started": phase = "waiting"; break;
      case "tool_call_requested": case "tool_call": case "tool_execution_started": phase = "tool-executing"; break;
      case "server_tool_content":
        if (isActiveServerToolContentFrame(frame)) phase = "tool-executing";
        else if (isTerminalServerToolContentFrame(frame)) phase = "waiting";
        else return false;
        break;
      case "tool_result_received": case "tool_execution_completed": case "reasoning_complete": phase = "waiting"; break;
      case "reasoning_delta": case "text_delta": phase = "generating"; break;
      case "text_complete": case "interaction_complete": case "interaction_failed":
      case "run_completed": case "run_failed": case "message_delivery_failed": phase = null; terminal = true; break;
      case "system_notice":
        if (!systemNoticeClearsBusyState(frame)) { this.terminal = false; return false; }
        phase = null; terminal = true; break;
      case "turn_completed": {
        const data = frame.data && typeof frame.data === "object" ? frame.data as Record<string, unknown> : {};
        const reason = data.stop_reason ?? data.stopReason;
        if (reason === "tool_use") { this.terminal = false; return false; }
        phase = null; terminal = true; break;
      }
      default: return false;
    }
    const owner = this.owner(frame);
    if (!owner) return false;
    // Replayed starts do not become current again after their run settles.
    // History associations never populate these explicit start observations.
    if (frame.event === "run_started" && frame.runId && owner.runStartOrder.has(frame.runId)) {
      this.refreshOwner(owner, true);
      return true;
    }
    const order = ++this.order;
    let ownerPhaseEvent = !frame.runId;
    switch (frame.event) {
      case "user_input":
        ownerPhaseEvent = true;
        owner.inputOpen = !terminalUser;
        if (terminalUser) owner.auxiliaryOpen = false;
        break;
      case "interaction_started": ownerPhaseEvent = true; owner.interactionOpen = true; break;
      case "run_started":
        if (frame.runId) {
          owner.runStartOrder.set(frame.runId, order);
          owner.openRuns.add(frame.runId);
        } else owner.unidentifiedRunOpen = true;
        break;
      case "interaction_complete": case "interaction_failed":
        if (frame.runId) {
          const started = owner.runStartOrder.get(frame.runId);
          const closesCurrent = started !== undefined
            && [...owner.runStartOrder.values()].every(value => value <= started)
            && ![...owner.runPhases].some(([id, value]) => id !== frame.runId
              && !owner.runStartOrder.has(id) && value.phase !== null && value.order > started);
          owner.openRuns.delete(frame.runId);
          if (owner.runPhases.get(frame.runId)?.phase !== null) owner.runPhases.delete(frame.runId);
          // Native run terminals are emitted as interaction terminals with
          // both IDs. The latest explicit start identifies the current run;
          // its terminal closes the interaction, an older terminal does not.
          // A partial history may omit a newer run's start: retain its observed
          // phase without manufacturing an open-run reservation for it.
          if (!closesCurrent && (owner.openRuns.size || owner.unidentifiedRunOpen
            || [...owner.runPhases.values()].some(value => value.phase !== null))) break;
        }
        // A run-less terminal settles the interaction, as does its last run.
        // falls through
      case "message_delivery_failed": case "system_notice":
        ownerPhaseEvent = true;
        owner.interactionOpen = owner.unidentifiedRunOpen = owner.inputOpen = owner.auxiliaryOpen = false;
        owner.openRuns.clear(); owner.runPhases.clear(); break;
      case "run_completed": case "run_failed":
        if (frame.runId) {
          owner.openRuns.delete(frame.runId);
          if (owner.runPhases.get(frame.runId)?.phase !== null) owner.runPhases.delete(frame.runId);
        }
        else owner.unidentifiedRunOpen = false;
        break;
      case "text_complete": case "text_delta": break;
      case "turn_completed": owner.inputOpen = owner.auxiliaryOpen = false; break;
      default: owner.auxiliaryOpen = true; break;
    }
    const hasRun = owner.unidentifiedRunOpen || owner.openRuns.size > 0;
    if (frame.event === "text_complete" || frame.event === "turn_completed") phase = owner.interactionOpen || hasRun ? "waiting" : null;
    if (frame.event === "run_completed" || frame.event === "run_failed"
      || ((frame.event === "interaction_complete" || frame.event === "interaction_failed") && !ownerPhaseEvent)) {
      // Observed run phases and explicit run starts are different facts. A
      // sibling's live text survives even when its start was not retained.
      if ((!owner.openRuns.size && ![...owner.runPhases.values()].some(value => value.phase !== null)
        && !owner.unidentifiedRunOpen) || !frame.runId) {
        owner.ownerPhase = { phase: owner.interactionOpen ? "waiting" : null, order };
      }
      this.selectOwnerPhase(owner);
    } else {
      const contribution = { phase: phase ?? null, order };
      if (frame.runId && !ownerPhaseEvent) {
        owner.runPhases.set(frame.runId, contribution);
      } else owner.ownerPhase = contribution;
      if (contribution.phase === null) this.selectOwnerPhase(owner);
      else { owner.phase = contribution.phase; owner.order = contribution.order; }
    }
    this.refreshOwner(owner);
    this.terminal = terminal && this.phase === null;
    return true;
  }
}

export function inferResponsePhaseFromFrames(frames: ConsoleFrame[], fallback: ResponsePhase = null): ResponsePhase {
  const projection = new ConsoleActivityProjection(fallback);
  for (const frame of frames) projection.fold(frame);
  return projection.phase;
}

function isTerminalUserInputStatus(status?: string): boolean {
  return status === "completed" || status === "delivery_failed" || status === "failed";
}

export function resolvePanelResponsePhase(args: {
  frames: ConsoleFrame[];
  serverPhase?: ResponsePhase;
  localPhase?: ResponsePhase;
  hasLocalPhase?: boolean;
}): ResponsePhase {
  if (args.hasLocalPhase) {
    return args.localPhase ?? null;
  }
  if (args.frames.length > 0) {
    const localPhase = inferResponsePhaseFromFrames(args.frames, null);
    if (args.serverPhase && localPhase === null && !latestRoutableFrameIsTerminal(args.frames)) {
      return args.serverPhase;
    }
    return localPhase;
  }
  return args.serverPhase ?? null;
}

function latestRoutableFrameIsTerminal(frames: ConsoleFrame[]): boolean {
  const projection = new ConsoleActivityProjection();
  for (const frame of frames) projection.fold(frame);
  return projection.terminal;
}

export function buildConversationViewState(args: {
  memberId: string;
  agentLabel: string;
  agent?: ConsoleAgent | null;
  entries: ConversationTimelineEntry[];
}): ConversationViewState {
  const groups = groupConversationTimelineEntries(args.entries);
  const suggestions = buildQuickPromptSuggestions(args.agent ?? null);
  return {
    conversationId: args.memberId || "console",
    title: args.agentLabel,
    entries: conversationPresentationRows(args.entries),
    groups,
    turnDiff: null,
    emptyState: args.entries.length === 0 ? {
      title: args.agentLabel,
      subtitle: "Send a message to start the conversation.",
      ...(suggestions.length ? { suggestions } : {}),
    } : null,
  };
}

export function buildActivityRailViewState(args: {
  agents: ConsoleAgent[];
  eventFrames: ConsoleFrame[];
  filterPresets?: ActivityFilterPreset[];
  activePresetId?: string;
}): ConsoleActivityRailViewState {
  const presets = args.filterPresets || [];
  const activePreset = presets.find((preset) => preset.id === args.activePresetId) || null;
  const agentByIdentity = new Map<string, ConsoleAgent>();
  const watchedIdentities = new Set<string>();
  const criticalIdentities = new Set<string>();

  for (const agent of args.agents) {
    if (agent.identity) agentByIdentity.set(agent.identity, agent);
    agentByIdentity.set(agent.member_id, agent);
    if (agent.watched && (agent.identity || agent.member_id)) {
      watchedIdentities.add(agent.identity || agent.member_id);
    }
    if (agent.alertLevel === "critical" && (agent.identity || agent.member_id)) {
      criticalIdentities.add(agent.identity || agent.member_id);
    }
  }

  const filteredFrames = args.eventFrames.filter((frame) => {
    if (ACTIVITY_HIDDEN_EVENTS.has(frame.event)) {
      return false;
    }
    if (frame.sourceKind === "session_history") {
      return false;
    }
    const frameIdentity = frame.identity?.trim();
    if (!activePreset) return true;
    if (activePreset.watchedOnly && frameIdentity && !watchedIdentities.has(frameIdentity)) {
      return false;
    }
    if (activePreset.alertLevels?.length && frameIdentity) {
      const agent = agentByIdentity.get(frameIdentity);
      if (!agent?.alertLevel || !activePreset.alertLevels.includes(agent.alertLevel)) {
        return false;
      }
    }
    if (activePreset.eventTypeFilter?.length && !activePreset.eventTypeFilter.includes(frame.event)) {
      return false;
    }
    return true;
  });

  const pulseItems: ConsoleActivityPulseItem[] = filteredFrames
    .slice(0, 200)
    .map((frame, index) => {
      const frameIdentity = frame.identity?.trim();
      const agent = frameIdentity ? agentByIdentity.get(frameIdentity) : null;
      const ts = typeof frame.timestampMs === "number"
        ? new Date(frame.timestampMs).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })
        : "";
      return {
        id: `event:${frame.id || index}`,
        title: agent?.label || frameIdentity || frame.event || "event",
        line: summarizeFrameData(frame.data).slice(0, 120) || frame.event,
        meta: `${frame.event}${ts ? ` · ${ts}` : ""}`,
        ...(agent ? { focusId: agent.member_id } : {}),
      };
    });

  return {
    panels: [
      {
        id: "pulse",
        kind: "pulse" as const,
        title: "Activity",
        actions: presets.map((preset) => ({
          id: preset.id,
          label: preset.label,
          active: preset.id === (activePreset?.id || "all"),
        })),
        items: pulseItems,
        emptyText: "No events yet",
      },
    ],
  };
}
