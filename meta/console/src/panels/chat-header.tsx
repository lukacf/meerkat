// The chat pane's header (console v2 spec B17): who the conversation is
// with, whether the agent is working now and on what, and the actions on
// it. Details, Fork to side chat and Stop run stay in view as kit buttons;
// Respawn and Retire sit in the header's overflow menu and ask before they
// act. Status words come from the shared agent status rules, so the header
// says what the sidebar, the roster and Health say.

import React from "react";
import type { ConversationTimelineEntry } from "@console-core";
import {
  AgentAvatar,
  Button,
  OverflowMenu,
  RelativeTime,
  StatusMark,
  StatusPill,
  formatClockTime,
  formatDuration,
  humanizeEnum,
  type MenuEntry,
  type Tone,
} from "@console-components";
import type { ConsoleAgent } from "../types";
import { responsePhaseWords } from "../lib/agents";
import { agentHealth, isAgentWorking } from "../lib/agent-status";
import { useActivityLedger } from "../lib/activity-ledger";
import { Icon } from "../icon";

export type ChatPhase = "waiting" | "tool-executing" | "generating" | null;

/** The tool call the agent is running now, as the transcript shows it. */
export interface ChatRunningTool {
  /** The tool's name as the agent called it ("web_search"). */
  name: string;
  /** The call's id, which ties it to the console's open tool span. */
  callId: string;
  /** For a message to another agent: that agent's name. */
  peer?: string;
}

const PEER_TOOL_VERBS: Readonly<Record<string, string>> = {
  send_message: "Messaging",
  send_request: "Asking",
  send_response: "Replying to",
  reply_to_peer: "Replying to",
};

/** How many entries from the end the running tool is looked for in. */
const RUNNING_TOOL_SCAN = 40;

/**
 * The newest tool call the transcript still shows as pending: the call the
 * agent is running while its phase is "tool-executing". Null when there is
 * none in the recent entries.
 */
export function runningToolOf(entries: readonly ConversationTimelineEntry[]): ChatRunningTool | null {
  const stop = Math.max(0, entries.length - RUNNING_TOOL_SCAN);
  for (let index = entries.length - 1; index >= stop; index -= 1) {
    const entry = entries[index];
    if (entry.kind !== "message" || !entry.blocks) continue;
    for (let blockIndex = entry.blocks.length - 1; blockIndex >= 0; blockIndex -= 1) {
      const block = entry.blocks[blockIndex];
      if (block.type !== "tool-call" || block.status !== "pending" || block.peerIncoming) continue;
      const peer = (block.peerDisplayLabel || block.peerTarget || "").trim();
      return { name: block.name, callId: block.toolCallId, ...(peer ? { peer } : {}) };
    }
  }
  return null;
}

/** What the agent is doing now, in words, with when it began. */
export interface ChatLiveActivity {
  /** "Writing a reply", "Running web search", "Messaging Billing". */
  text: string;
  /** The tool's own name, for the tooltip. */
  detail?: string;
  /** When the activity began (epoch ms), for the elapsed time. */
  sinceMs?: number;
}

function capitalize(words: string): string {
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : words;
}

/**
 * The live line: the response phase in the words the composer footer and
 * the roster use, a running tool by its name ("Running web search"), a
 * message to another agent by its recipient ("Messaging Billing"), or
 * "Working" for an open run without a phase. Null when the agent is not
 * working. The elapsed time comes only from the console's own record of
 * when the tool or the run started, never from a message's time.
 */
export function chatLiveActivity(input: {
  phase: ChatPhase;
  working: boolean;
  tool?: ChatRunningTool | null;
  toolStartMs?: number;
  runStartMs?: number;
}): ChatLiveActivity | null {
  const { phase, working, tool } = input;
  if (phase === "tool-executing") {
    if (!tool) return { text: capitalize(responsePhaseWords(phase)), sinceMs: input.toolStartMs };
    const verb = PEER_TOOL_VERBS[tool.name];
    const text = verb
      ? `${verb} ${tool.peer || "another agent"}`
      : `Running ${humanizeEnum(tool.name).toLowerCase() || "a tool"}`;
    return { text, detail: tool.name, sinceMs: input.toolStartMs };
  }
  if (phase) return { text: capitalize(responsePhaseWords(phase)) };
  if (working) return { text: "Working", sinceMs: input.runStartMs };
  return null;
}

/** The header's status pill: what the agent is doing, or what is wrong with it. */
export interface ChatAgentStatus {
  tone: Tone;
  word: string;
  /** Working now: the avatar wears the live ring. */
  working: boolean;
}

/**
 * The status pill and the live ring, from the shared status rules: a
 * problem (broken, stalled, needs repair, degraded) or a retirement names
 * itself; otherwise a working agent reads "Working" (never "Unknown", even
 * while the team runtime cannot judge a busy member's health), a quiet
 * healthy one "Idle", and one the console has not observed "Not observed".
 */
export function chatAgentStatus(agent: ConsoleAgent | null, phase: ChatPhase, openRun: boolean): ChatAgentStatus | null {
  if (!agent) return phase ? { tone: "live", word: "Working", working: true } : null;
  const health = agentHealth(agent, { openRun });
  const retired = health.kind === "retired" || health.kind === "retiring";
  const working = !retired && (phase !== null || isAgentWorking(agent, { openRun }));
  if (health.problem || retired) return { tone: health.tone, word: health.word, working };
  if (working) return { tone: "live", word: "Working", working };
  if (health.kind === "unobserved") return { tone: health.tone, word: health.word, working };
  return { tone: "neutral", word: "Idle", working };
}

/**
 * "Role · group": the role in words and the agent's group, leaving out a
 * role that only repeats the name and a group that only repeats the role
 * ("Coordinator · Coordinators" reads "Coordinator").
 */
export function chatRoleGroupWords(agent: ConsoleAgent | null, agentLabel: string): string {
  if (!agent) return "";
  const role = humanizeEnum(agent.role || "");
  const name = agentLabel.trim().toLowerCase();
  const roleWords = role && role.toLowerCase() !== name ? role : "";
  const group = (agent.group || agent.labels?.group || "").trim();
  const singular = (value: string) => value.toLowerCase().replace(/s$/, "");
  const groupWords = group && group.toLowerCase() !== name && (!role || singular(group) !== singular(role)) ? group : "";
  return [roleWords, groupWords].filter(Boolean).join(" · ");
}

/** Now, ticking every second for the first minute after `sinceMs`, then every 15 s. */
function useElapsedClock(sinceMs: number | undefined): number {
  const [now, setNow] = React.useState(() => Date.now());
  React.useEffect(() => {
    if (sinceMs === undefined) return undefined;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const schedule = () => {
      const elapsed = Date.now() - sinceMs;
      const delay = elapsed < 60_000 ? 1_000 - (((elapsed % 1_000) + 1_000) % 1_000) : 15_000;
      timer = setTimeout(() => {
        setNow(Date.now());
        schedule();
      }, Math.max(250, delay));
    };
    schedule();
    return () => {
      if (timer !== undefined) clearTimeout(timer);
    };
  }, [sinceMs]);
  return now;
}

function Elapsed({ sinceMs }: { sinceMs: number }) {
  const now = useElapsedClock(sinceMs);
  return (
    <span className="conv__elapsed" title={`Since ${formatClockTime(sinceMs)}`}>
      {formatDuration(Math.max(0, now - sinceMs))}
    </span>
  );
}

/** The open span of the running tool: the one tied to its call id, else the only one open. */
function openToolStart(
  spans: ReadonlyArray<{ key: string; startMs?: number; endMs?: number }>,
  callId: string | undefined,
): number | undefined {
  const open = spans.filter((span) => span.endMs === undefined && span.startMs !== undefined);
  if (callId) {
    const exact = open.find((span) => span.key.endsWith(`:${callId}`));
    if (exact) return exact.startMs;
  }
  return open.length === 1 ? open[0].startMs : undefined;
}

type LifecycleAction = "respawn" | "retire";

interface HeaderActionProps {
  id: string;
  label: string;
  icon: string;
  identity: string;
  onClick: () => void;
  disabledReason?: string | null;
}

/** A header action: a kit button with its sprite icon, named for its target. */
function HeaderAction({ id, label, icon, identity, onClick, disabledReason }: HeaderActionProps) {
  return (
    <Button
      className="conv__action"
      icon={<Icon name={icon} />}
      onClick={onClick}
      disabled={Boolean(disabledReason)}
      aria-label={label}
      title={disabledReason ? `${label} - ${disabledReason}` : `${label} - ${identity}`}
      data-testid={`conv-action:${id}`}
    >
      <span className="conv__action-label">{label}</span>
    </Button>
  );
}

const CONSEQUENCE: Readonly<Record<LifecycleAction, string>> = {
  respawn: "It restarts and keeps its conversation.",
  retire: "It stops and leaves the team.",
};

/** The inline confirmation for Respawn and Retire, under the header row. */
function ConfirmStrip({
  action,
  label,
  agentLabel,
  onConfirm,
  onCancel,
}: {
  action: LifecycleAction;
  label: string;
  agentLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirmRef = React.useRef<HTMLButtonElement>(null);
  const questionId = React.useId();
  React.useEffect(() => {
    confirmRef.current?.focus();
  }, [action]);
  return (
    <div
      className="conv__confirm"
      role="group"
      aria-labelledby={questionId}
      data-action={action}
      data-testid={`conv-confirm:${action}`}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          onCancel();
        }
      }}
    >
      <StatusMark tone={action === "retire" ? "warn" : "info"} className="conv__confirm-mark" />
      <p className="conv__confirm-text" id={questionId}>
        <strong>{label} {agentLabel}?</strong> {CONSEQUENCE[action]}
      </p>
      <div className="conv__confirm-actions">
        <Button
          ref={confirmRef}
          size="sm"
          variant={action === "retire" ? "danger" : "default"}
          onClick={onConfirm}
          data-testid={`conv-confirm-accept:${action}`}
        >
          {label}
        </Button>
        <Button size="sm" variant="ghost" onClick={onCancel} data-testid={`conv-confirm-cancel:${action}`}>
          Cancel
        </Button>
      </div>
    </div>
  );
}

export interface ChatHeaderProps {
  identity: string;
  agentLabel: string;
  agent: ConsoleAgent | null;
  phase: ChatPhase;
  /** The tool call the transcript shows running (while the phase is "tool-executing"). */
  toolName?: string;
  toolCallId?: string;
  toolPeer?: string;
  /** Console panes use the compact spacing. */
  compact?: boolean;
  onInspect?: () => void;
  inspectLabel: string;
  onForkSideChat?: () => void;
  forkSideChatLabel: string;
  forkSideChatDisabledReason?: string | null;
  onStopRun?: () => void;
  stopRunLabel: string;
  onRespawn?: () => void;
  respawnLabel: string;
  onRetire?: () => void;
  retireLabel: string;
}

/** The chat header: the agent, its status and live line, and its actions. */
export const ChatHeader = React.memo(function ChatHeader({
  identity,
  agentLabel,
  agent,
  phase,
  toolName,
  toolCallId,
  toolPeer,
  compact = false,
  onInspect,
  inspectLabel,
  onForkSideChat,
  forkSideChatLabel,
  forkSideChatDisabledReason,
  onStopRun,
  stopRunLabel,
  onRespawn,
  respawnLabel,
  onRetire,
  retireLabel,
}: ChatHeaderProps) {
  const ledger = useActivityLedger();
  const live = ledger.liveState(identity);
  const openRun = live.openRun !== null;
  const status = chatAgentStatus(agent, phase, openRun);
  const working = status?.working ?? false;
  const tool = toolName && toolCallId ? { name: toolName, callId: toolCallId, ...(toolPeer ? { peer: toolPeer } : {}) } : null;
  const activity = working
    ? chatLiveActivity({
        phase,
        working,
        tool,
        toolStartMs: phase === "tool-executing" ? openToolStart(ledger.toolSpans(identity), toolCallId) : undefined,
        runStartMs: live.openRun?.startMs,
      })
    : null;
  const roleGroup = chatRoleGroupWords(agent, agentLabel);
  const lastActiveMs = agent?.last_activity_ms ?? live.lastActivityMs;

  // Respawn and Retire ask first. A confirmation whose action went away (the
  // affordance was withdrawn) closes.
  const [confirm, setConfirm] = React.useState<LifecycleAction | null>(null);
  const confirmAction = confirm === "respawn" ? onRespawn : confirm === "retire" ? onRetire : undefined;
  React.useEffect(() => {
    if (confirm && !confirmAction) setConfirm(null);
  }, [confirm, confirmAction]);
  React.useEffect(() => {
    setConfirm(null);
  }, [identity]);
  const frameRef = React.useRef<HTMLDivElement>(null);
  const closeConfirm = React.useCallback(() => {
    setConfirm(null);
    frameRef.current?.querySelector<HTMLButtonElement>(".conv__menu")?.focus();
  }, []);

  const menuItems: MenuEntry[] = [];
  if (onRespawn) {
    menuItems.push({ kind: "item", id: "respawn", label: `${respawnLabel}…`, textValue: respawnLabel, onSelect: () => setConfirm("respawn"), testId: "conv-action:respawn" });
  }
  if (onRetire) {
    menuItems.push({ kind: "item", id: "retire", label: `${retireLabel}…`, textValue: retireLabel, destructive: true, onSelect: () => setConfirm("retire"), testId: "conv-action:retire" });
  }

  return (
    <div className="conv__head-frame" ref={frameRef}>
      <div
        className={`conv__head${compact ? " conv__head--compact" : ""}`}
        data-working={working ? "true" : undefined}
        data-testid={`conv-head:${identity}`}
      >
        <AgentAvatar name={agentLabel} size={28} live={working} className="conv__avatar" />
        <div className="conv__target">
          <div className="conv__title-row">
            <span className="conv__title" title={identity}>{agentLabel}</span>
            {roleGroup ? <span className="conv__role">{roleGroup}</span> : null}
            {status ? (
              <StatusPill tone={status.tone} className="conv__status" testId={`conv-status:${identity}`}>
                {status.word}
              </StatusPill>
            ) : null}
          </div>
          <div className="conv__live" data-live={activity ? "true" : undefined} data-testid={`conv-live:${identity}`}>
            {activity ? (
              <>
                <StatusMark tone="live" className="conv__live-mark" />
                <span className="conv__live-text" title={activity.detail}>{activity.text}</span>
                {activity.sinceMs !== undefined ? (
                  <>
                    <span className="conv__sep" aria-hidden="true">·</span>
                    <Elapsed sinceMs={activity.sinceMs} />
                  </>
                ) : null}
              </>
            ) : lastActiveMs ? (
              <RelativeTime value={lastActiveMs} prefix="Last active" className="conv__last-active" />
            ) : (
              <span className="conv__last-active">No activity seen yet</span>
            )}
          </div>
        </div>
        <div className="conv__actions">
          {onInspect ? <HeaderAction id="details" label={inspectLabel} icon="i-info" identity={identity} onClick={onInspect} /> : null}
          {onForkSideChat ? (
            <HeaderAction
              id="fork-side-chat"
              label={forkSideChatLabel}
              icon="i-branch"
              identity={identity}
              onClick={onForkSideChat}
              disabledReason={forkSideChatDisabledReason}
            />
          ) : null}
          {onStopRun ? <HeaderAction id="stop-run" label={stopRunLabel} icon="i-stop" identity={identity} onClick={onStopRun} /> : null}
          {menuItems.length > 0 ? (
            <OverflowMenu
              label={`More actions for ${agentLabel}`}
              items={menuItems}
              quiet
              triggerClassName="conv__menu"
              triggerTestId="conv-menu"
              testId={`conv-menu-items:${identity}`}
            />
          ) : null}
        </div>
      </div>
      {confirm && confirmAction ? (
        <ConfirmStrip
          action={confirm}
          label={confirm === "respawn" ? respawnLabel : retireLabel}
          agentLabel={agentLabel}
          onCancel={closeConfirm}
          onConfirm={() => {
            setConfirm(null);
            confirmAction();
          }}
        />
      ) : null}
    </div>
  );
});
