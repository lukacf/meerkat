// The Signals rail's model (console v2 spec B6): what deserves attention
// now, as signals people read at a glance. Turns, replies, peer messages,
// approvals waiting for a decision, memory warnings, lifecycle changes and
// any other warning or failure the shared classifier finds; a turn's
// events fold into one group. Severity is the classifier's (payload aware),
// and every signal knows where it leads (its chat, the approval, the memory
// record, the agent's details, Routing). Pure module (no React): the rail's
// node test bundle loads it.

import { memberKickoffNotice, memberKickoffPhaseLabel, type MemberKickoffNotice } from "../../../../packages/console-core/src/member-kickoff";
import {
  EMPTY_AGENT_DIRECTORY,
  agentDisplayName,
  type AgentDirectory,
} from "../../../../packages/console-components/src/panel-kit/agents";
import { humanizeEnum } from "../../../../packages/console-components/src/panel-kit/format";
import { stripPeerTransportScaffold } from "../../lib/adapters";
import { describeFailure } from "../../lib/failure-summary";
import { classifyFrame, failureWords, frameSentence, plainSnippetText } from "../../lib/frame-classifier";
import { describeMemoryEventForPeople, isOpenLoopNudge } from "../../lib/memory-event-words";
import { turnSenderLine } from "../../lib/turn-origin";
import type { ConsoleFrame, ConsoleRailFilterPresetConfig } from "../../types";

export type SignalSeverity = "critical" | "warning" | "info";

/** Where a signal leads (spec A4.7). */
export type SignalTarget =
  | { kind: "chat"; identity: string; interactionId?: string }
  | { kind: "agent"; identity: string }
  | { kind: "approval"; pendingId: string }
  | { kind: "memory"; frame: ConsoleFrame; realm?: string; recordId?: string }
  | { kind: "routing"; routeKey?: string };

export interface Signal {
  id: string;
  severity: SignalSeverity;
  label: string;
  detail: string;
  agent: string;
  atMs: number;
  raw: ConsoleFrame;
  target?: SignalTarget;
  /** An approval waiting for a decision: the rail offers Review. */
  approval?: PendingApprovalLike;
}

export interface SignalGroup {
  id: string;
  severity: SignalSeverity;
  title: string;
  detail: string;
  agent: string;
  atMs: number;
  items: Signal[];
  target?: SignalTarget;
}

/** The fields of a pending approval the rail reads. */
export interface PendingApprovalLike {
  pendingId: string;
  action: string;
  actorId?: string;
  rationale?: string;
  riskTier?: string;
  status?: string;
  createdAtMs?: number;
  deadlineAtMs?: number;
  origin?: { identity: string; interactionId?: string };
}

export const DEFAULT_FILTER_PRESETS: ConsoleRailFilterPresetConfig[] = [
  { id: "all", label: "All" },
  { id: "warning", label: "Attention", alertLevels: ["warning", "critical"] },
  { id: "critical", label: "Critical", alertLevels: ["critical"] },
];

/** The experience's activity feed presets (alert levels "elevated" and
 * "critical", event types) in the rail's terms. */
export function railPresetsFromActivityFeed(
  presets: ReadonlyArray<{ id: string; label: string; watchedOnly?: boolean; alertLevels?: readonly string[]; eventTypeFilter?: readonly string[] }> | null | undefined,
): ConsoleRailFilterPresetConfig[] | undefined {
  const usable = (presets ?? []).filter((preset) => preset && preset.id && preset.label);
  if (!usable.length) return undefined;
  return usable.map((preset) => {
    const levels = new Set<string>();
    for (const level of preset.alertLevels ?? []) {
      if (level === "elevated" || level === "warning") {
        levels.add("warning");
        levels.add("critical");
      } else if (level === "critical") levels.add("critical");
    }
    return {
      id: preset.id,
      label: preset.label,
      ...(preset.watchedOnly ? { watchedOnly: true } : {}),
      ...(levels.size ? { alertLevels: [...levels] } : {}),
      ...(preset.eventTypeFilter?.length ? { eventTypes: [...preset.eventTypeFilter] } : {}),
    } as ConsoleRailFilterPresetConfig;
  });
}

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
const LIFECYCLE_EVENTS = new Set(["member_retired", "identity_retired", "identity_respawned", "identity_reset", "identity_deleted"]);

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
  // Model markdown reads as plain words (the classifier's own rule).
  const normalized = plainSnippetText(value)
    .replace(/\*\*([^*]+)\*\*/g, "$1")
    .replace(/`([^`]+)`/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (normalized.length <= max) return normalized;
  return `${normalized.slice(0, Math.max(0, max - 1)).trimEnd()}...`;
}

type NameResolver = (value: string) => string;

/** Names agents the way every surface does: the directory's display name
 * when it knows the agent, else the name the shared naming function gives
 * the key ("identity:support-triage" reads "Support triage"), and "System"
 * for console events. */
export function signalNameResolver(directory: AgentDirectory = EMPTY_AGENT_DIRECTORY): NameResolver {
  return (value) => {
    if (!value || value === "_system") return "System";
    const label = directory.lookup(value)?.label?.trim();
    return label || agentDisplayName(value, directory);
  };
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
  return blocks
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
    // intent summary, never the raw envelope. A typed lifecycle notice's
    // content is model-facing notice text.
    const names = [textFromValue(peer.display_name), textFromValue(peer.id)].filter(Boolean);
    const content = block.kind === "lifecycle" ? "" : stripPeerTransportScaffold(textFromValue(block.content), names);
    const detail = content || textFromValue(block.summary) || textFromValue(block.intent) || textFromValue(block.payload);
    if (detail) details.push(detail);
  }
  return { targets, detail: details.join(" "), incoming };
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

/** The classifier's severity in the rail's words. */
export function signalSeverity(frame: ConsoleFrame, directory?: AgentDirectory): SignalSeverity {
  const severity = classifyFrame(frame, directory).severity;
  return severity === "crit" ? "critical" : severity === "warn" ? "warning" : "info";
}

function memoryTarget(frame: ConsoleFrame): SignalTarget {
  const data = recordOf(frame.data);
  const realm = typeof data.realm === "string" && data.realm.trim() ? data.realm.trim() : undefined;
  const recordId = typeof data.record_id === "string" && data.record_id.trim() ? data.record_id.trim() : undefined;
  return { kind: "memory", frame, ...(realm ? { realm } : {}), ...(recordId ? { recordId } : {}) };
}

function chatTarget(frame: ConsoleFrame): SignalTarget | undefined {
  const identity = frame.identity?.trim();
  if (!identity || identity === "_system") return undefined;
  return { kind: "chat", identity, ...(frame.interactionId ? { interactionId: frame.interactionId } : {}) };
}

function signalFromFrame(
  frame: ConsoleFrame,
  displayName: NameResolver,
  directory: AgentDirectory | undefined,
  turnLabels?: ReadonlyMap<string, string>,
): Signal | null {
  const data = recordOf(frame.data);
  const classified = classifyFrame(frame, directory);
  const severity: SignalSeverity = classified.severity === "crit" ? "critical" : classified.severity === "warn" ? "warning" : "info";
  const base = {
    id: frame.id || `${frame.event}:${frame.timestampMs || 0}`,
    severity,
    agent: agentFor(frame),
    atMs: frame.timestampMs || 0,
    raw: frame,
  };

  if (frame.event.startsWith("memory.")) return memorySignal(frame, data, base, displayName);

  if (severity === "critical") {
    return {
      ...base,
      label: frame.event === "interaction_failed" ? "Agent turn failed" : classified.title || humanizeEnum(frame.event),
      detail: truncate(
        failureWords(describeFailure(data, ""))
          || textFromValue(data.error ?? data.reason ?? data.message)
          || classified.snippet
          || "Needs attention",
      ),
      target: chatTarget(frame),
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
      // Name the real sender from the typed origin. A runtime's
      // interaction_started carries none: it takes the label of its turn's
      // user_input when there is one, so the two still collapse into one
      // signal, and otherwise says the agent started a turn.
      const hasOrigin = Boolean(textFromValue(data.origin_kind) || textFromValue(data.origin));
      const name = displayName(base.agent);
      const label = frame.event === "user_input" || hasOrigin
        ? turnSenderLine(data, name)
        : turnLabels?.get(frame.interactionId || "") ?? `${name} started a turn`;
      return {
        ...base,
        id: `user:${frame.id || frame.interactionId || frame.timestampMs || request}`,
        label,
        detail: truncate(request),
        target: chatTarget(frame),
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
          target: { kind: "agent", identity: kickoff.member },
        };
      }
      const peer = comms.targets.map(displayName).join(", ");
      return {
        ...base,
        id: `comms:${frame.id || frame.interactionId || frame.timestampMs || peer}`,
        label: `${comms.incoming ? "Received from" : "Sent to"} ${peer}`,
        detail: truncate(comms.detail || "Peer comms"),
        target: chatTarget(frame),
      };
    }
    case "interaction_complete": {
      const reply = sessionHistoryAssistantReply(frame, data);
      if (!isMeaningfulReply(reply)) return null;
      return {
        ...base,
        label: `${displayName(base.agent)} replied`,
        detail: truncate(reply),
        target: chatTarget(frame),
      };
    }
    case "assistant_image":
    case "assistant_image_appended": {
      return {
        ...base,
        id: `image:${blobKey(frame)}`,
        label: `${displayName(base.agent)} generated image`,
        detail: textFromValue(data.prompt ?? recordOf(data.image).prompt ?? recordOf(data.image).alt) || "Generated image attached",
        target: chatTarget(frame),
      };
    }
    case "tool_call_requested": {
      const name = typeof data.name === "string" ? data.name : "";
      if (!PEER_TOOLS.has(name)) break;
      const args = recordOf(data.args);
      const target = classified.target || peerTarget(args);
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
        target: chatTarget(frame),
      };
    }
    case "gating_decision":
      return {
        ...base,
        label: approvalLabel(String(data.decision || "")),
        detail: truncate(textFromValue(data.reason) || "Approval decision recorded"),
        target: typeof data.pending_id === "string" && data.pending_id
          ? { kind: "approval", pendingId: data.pending_id }
          : chatTarget(frame),
      };
    case "state_changed": {
      const state = String(data.state || data.new_state || "");
      return {
        ...base,
        label: "State changed",
        detail: state ? `Now ${humanizeEnum(state).toLowerCase()}` : "Lifecycle change",
        ...(frame.identity && frame.identity !== "_system" ? { target: { kind: "agent" as const, identity: frame.identity } } : {}),
      };
    }
    case "route_changed":
      return { ...base, label: "Route changed", detail: truncate(textFromValue(data.reason) || "Routing updated"), target: { kind: "routing" } };
    default:
      break;
  }
  if (LIFECYCLE_EVENTS.has(frame.event)) {
    const identity = frame.identity?.trim();
    return {
      ...base,
      label: frame.event === "member_retired" ? "Member retired" : classified.title,
      detail: truncate(frameSentence(classified, displayName) + (classified.snippet ? ` · ${classified.snippet}` : "")) || "Lifecycle change",
      ...(identity && identity !== "_system" ? { target: { kind: "agent" as const, identity } } : {}),
    };
  }
  // Any other warning the classifier finds is worth a look: a tool that
  // timed out, a budget nearly used, a failed hook.
  if (severity === "warning" && !classified.internal) {
    return {
      ...base,
      label: classified.title,
      detail: truncate(classified.snippet ? `${frameSentence(classified, displayName)} · ${classified.snippet}` : frameSentence(classified, displayName)),
      target: chatTarget(frame),
    };
  }
  return null;
}

/// Map a `memory.*` frame to a rail signal. Warning-severity events surface
/// operational issues (quarantined writes, taints, budget denials, blocked
/// hygiene, blocked quarantine releases, conflicts); info-severity events
/// surface routine progress. The remaining subtypes (dream start/skip,
/// hygiene proposed/applied/skipped, distill timeouts, non-tainted taint
/// transitions) are dropped from the rail.
function memorySignal(
  frame: ConsoleFrame,
  data: Record<string, unknown>,
  base: Omit<Signal, "label" | "detail">,
  displayName: NameResolver,
): Signal | null {
  // Agents by name and plain words; the technical line stays in the shared
  // describeMemoryTimelineEvent for the transcript and audits.
  const detail = truncate(describeMemoryEventForPeople(frame.event, data, { agentName: displayName }));
  const target = memoryTarget(frame);
  const warning = (label: string): Signal => ({ ...base, severity: "warning", label, detail, target });
  const info = (label: string): Signal => ({ ...base, severity: "info", label, detail, target });

  switch (frame.event) {
    case "memory.write.quarantined":
      return warning("Memory write quarantined");
    case "memory.taint.transition":
      return data.kind === "tainted" ? warning("Session became untrusted") : null;
    case "memory.budget.denied":
      return warning("Memory budget reached");
    case "memory.hygiene.blocked":
      return warning("Memory tidy-up blocked");
    case "memory.quarantine.release_blocked":
      return warning("Quarantine release blocked");
    case "memory.conflict.signal":
      return warning("Conflicting memories");
    case "memory.dream.completed":
      return info("Memory dream completed");
    case "memory.record.promoted":
      return info("Memory record moved");
    case "memory.harvest.completed":
      return info("Retired agent's memory summarized");
    case "memory.quarantine.verdict":
      // The steward's open-loop nudge rides the verdict event.
      return info(isOpenLoopNudge(frame.event, data) ? "Open loop needs follow-up" : "Quarantine verdict");
    case "memory.promotion.pending_gate":
      return info("Memory record waiting to move");
    default:
      // memory.dream.started/skipped, memory.hygiene.proposed/applied/skipped,
      // memory.distill.timed_out, and any other subtype are not shown.
      return null;
  }
}

/** A pending approval as a signal: an agent waits for a person. */
export function approvalSignal(approval: PendingApprovalLike, nowMs: number = Date.now()): Signal | null {
  if (approval.status && approval.status !== "pending") return null;
  if (approval.deadlineAtMs !== undefined && approval.deadlineAtMs < nowMs) return null;
  const actor = approval.actorId?.trim() || approval.origin?.identity?.trim() || "_system";
  const raw: ConsoleFrame = {
    id: `approval:${approval.pendingId}`,
    event: "approval_pending",
    identity: actor,
    ...(approval.origin?.interactionId ? { interactionId: approval.origin.interactionId } : {}),
    ...(approval.createdAtMs ? { timestampMs: approval.createdAtMs } : {}),
    data: { pending_id: approval.pendingId, action: approval.action, risk_tier: approval.riskTier },
  };
  return {
    id: `approval:${approval.pendingId}`,
    severity: "warning",
    label: "Approval requested",
    detail: truncate(approval.action || approval.rationale || "Waiting for a decision"),
    agent: actor,
    atMs: approval.createdAtMs ?? nowMs,
    raw,
    target: { kind: "approval", pendingId: approval.pendingId },
    approval,
  };
}

function groupKeyFor(signal: Signal): string {
  if (signal.approval) return `single:${signal.id}`;
  const interactionId = signal.raw.interactionId?.trim();
  if (interactionId) return `interaction:${interactionId}`;
  return `single:${signal.id}`;
}

function semanticSignalKey(signal: Signal): string {
  const canonical = (value: string) => value
    .replace(/\+\d+\s+-\d+\b/g, "")
    .replace(/[‘’]/g, "'")
    .replace(/[“”]/g, "\"")
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

function strongerSeverity(a: SignalSeverity, b: SignalSeverity): SignalSeverity {
  if (a === "critical" || b === "critical") return "critical";
  if (a === "warning" || b === "warning") return "warning";
  return "info";
}

function titleForGroup(items: Signal[]): string {
  if (items.length === 1) return items[0].label;
  // A burst of one kind of signal keeps its words; the rail shows the count.
  if (items.every((item) => item.label === items[0].label)) return items[0].label;
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

/** Where a group leads: its strongest item's target, a turn's chat. */
function targetForGroup(items: Signal[]): SignalTarget | undefined {
  const ranked = items.slice().sort((a, b) => rank(b.severity) - rank(a.severity));
  return ranked.find((item) => item.target)?.target;
}

function rank(severity: SignalSeverity): number {
  return severity === "critical" ? 2 : severity === "warning" ? 1 : 0;
}

/** How far apart the signals of one burst (13 tickets received) may be. */
const BURST_MS = 120_000;

function groupSignals(signals: Signal[]): SignalGroup[] {
  const groups: SignalGroup[] = [];
  const byId = new Map<string, SignalGroup>();
  const bursts = new Map<string, SignalGroup>();
  const burstGroups = new Set<SignalGroup>();
  for (const signal of signals) {
    let key = groupKeyFor(signal);
    // Signals of no turn that repeat (one agent, one kind, close together)
    // fold into one burst: newest first, so the burst's oldest item is last.
    if (key.startsWith("single:") && !signal.approval) {
      const burstKey = `${signal.agent}\u0000${signal.label}`;
      const burst = bursts.get(burstKey);
      const oldest = burst?.items[burst.items.length - 1];
      if (burst && oldest && oldest.atMs - signal.atMs <= BURST_MS) {
        key = burst.id;
        burstGroups.add(burst);
      }
    }
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
      existing.atMs = existing.items[0]?.atMs || signal.atMs;
      existing.target = targetForGroup(existing.items);
      continue;
    }
    const group: SignalGroup = {
      id: key,
      severity: signal.severity,
      title: titleForGroup([signal]),
      detail: detailForGroup([signal]),
      agent: signal.agent,
      atMs: signal.atMs,
      items: [signal],
      ...(signal.target ? { target: signal.target } : {}),
    };
    byId.set(key, group);
    groups.push(group);
    if (key.startsWith("single:") && !signal.approval) bursts.set(`${signal.agent}\u0000${signal.label}`, group);
  }
  // A burst keeps the id of its oldest signal, so new arrivals keep its row.
  for (const group of burstGroups) group.id = `burst:${group.items[group.items.length - 1].id}`;
  return groups;
}

/** Approval decisions in words: "Approval requested", "Approved". */
function approvalLabel(decision: string): string {
  const word = decision.trim().toLowerCase();
  if (word === "requested" || word === "pending") return "Approval requested";
  if (word === "approve" || word === "approved" || word === "auto_approved") return "Approved";
  if (word === "reject" || word === "rejected" || word === "denied") return "Approval refused";
  if (word === "escalate" || word === "escalated") return "Approval escalated";
  return word ? `Approval ${humanizeEnum(word).toLowerCase()}` : "Approval decision";
}

export interface SignalOptions {
  /** Approvals waiting for a decision (the approval resource's requests). */
  approvals?: readonly PendingApprovalLike[];
  nowMs?: number;
}

/**
 * The rail's groups, newest first: approvals waiting for a decision, then
 * the signals of the newest 260 frames (one per event, a turn's in one
 * group), at most 80.
 */
export function buildSignalGroups(
  frames: ConsoleFrame[],
  directory: AgentDirectory = EMPTY_AGENT_DIRECTORY,
  options: SignalOptions = {},
): SignalGroup[] {
  const displayName = signalNameResolver(directory);
  const seen = new Set<string>();
  const seenSemantic = new Set<string>();
  const next: Signal[] = [];
  const window = frames.slice(0, 260);
  // One logical event lists once: a live frame and its saved copy share the
  // classifier's event key, and the live copy speaks (it names the sender).
  const shown = new Map<string, ConsoleFrame>();
  for (const frame of window) {
    const key = classifyFrame(frame, directory).eventKey;
    const held = shown.get(key);
    if (!held || (held.sourceKind === "session_history" && frame.sourceKind !== "session_history")) shown.set(key, frame);
  }
  // Each turn's sender line, from its user_input frame.
  const turnLabels = new Map<string, string>();
  for (const frame of window) {
    if (frame.event !== "user_input" || !frame.interactionId) continue;
    turnLabels.set(frame.interactionId, turnSenderLine(recordOf(frame.data), displayName(agentFor(frame))));
  }
  for (const frame of window) {
    if (shown.get(classifyFrame(frame, directory).eventKey) !== frame) continue;
    const signal = signalFromFrame(frame, displayName, directory, turnLabels);
    if (!signal) continue;
    if (seen.has(signal.id)) continue;
    seen.add(signal.id);
    const semanticKey = semanticSignalKey(signal);
    if (seenSemantic.has(semanticKey)) continue;
    seenSemantic.add(semanticKey);
    next.push(signal);
    if (next.length >= 80) break;
  }
  const nowMs = options.nowMs ?? Date.now();
  const approvals = (options.approvals ?? [])
    .map((approval) => approvalSignal(approval, nowMs))
    .filter((signal): signal is Signal => signal !== null);
  const signals = [...approvals, ...next].sort((a, b) => b.atMs - a.atMs);
  return groupSignals(signals);
}

/** The groups the tests read: the frames alone, as the rail shows them. */
export function buildSignalGroupsForTest(frames: ConsoleFrame[], directory: AgentDirectory = EMPTY_AGENT_DIRECTORY): SignalGroup[] {
  return buildSignalGroups(frames, directory);
}

/** Whether a group matches a preset: watched agents, alert levels, event types. */
export function groupMatchesPreset(
  group: SignalGroup,
  preset: ConsoleRailFilterPresetConfig & { eventTypes?: readonly string[] },
  watchedIdentities?: ReadonlySet<string>,
): boolean {
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
  const eventTypes = preset.eventTypes;
  if (eventTypes?.length && !group.items.some((item) => eventTypes.includes(item.raw.event))) return false;
  return true;
}
