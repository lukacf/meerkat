import React from "react";
import type { IdentityInspectViewState } from "@console-core";
import type { ConsoleAgent } from "../types";
import { agentDisplayTitle } from "../lib/agents";
import { agentHealth, isAgentWorking, type AgentHealth } from "../lib/agent-status";
import { useActivityLedger, type LedgerRunSpan } from "../lib/activity-ledger";
import {
  DAY_MS,
  HOUR_MS,
  agentActivity,
  agentNowLine,
  countWords,
  hourLabel,
  seriesWords,
  shortSpan,
} from "../lib/agent-activity";
import { LifecycleConfirm, type ConfirmedLifecycleAction } from "./LifecycleConfirm";
import { useMeasuredWidth } from "../lib/use-element-width";
import { CopyButton } from "../../../packages/console-components/src/copy-button";
import {
  ActivityBars,
  AgentAvatar,
  AgentName,
  Button,
  EmptyState,
  EntityLink,
  EntityRow,
  FigureTile,
  GlanceStrip,
  KeyValueList,
  LoadingState,
  Notice,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  RelativeTime,
  StatusMark,
  StatusPill,
  TechnicalDetails,
  agentDisplayName,
  formatClockTime,
  humanizeEnum,
  plural,
  statusTone,
  useAgentDirectory,
  useConsoleNavigation,
  useShowTechnicalIds,
  type MenuEntry,
  type SummaryFact,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import { GraphView, StatusSquare } from "../../../packages/console-components/src/viz";
import type { VizEdge, VizNode } from "../../../packages/console-components/src/viz";

export interface AgentDetailsAction {
  label: string;
  onClick: () => void;
}

/** A work item the agent owns or has claimed (from the WorkGraph snapshot). */
export interface AgentWorkItem {
  id: string;
  title: string;
  /** open, ready, in_progress, blocked, completed, failed, cancelled */
  status: string;
  /** When the agent claimed it. */
  claimedAt?: string | null;
  priority?: string | null;
}

/** The inspect read as the console keeps it: the view state plus the facts
 * a newer gateway's answer carries that the view state has no field for. */
export type ConsoleInspectView = IdentityInspectViewState & {
  /** The runtime that holds the member. */
  runtime_key?: string;
  /** Who may see the agent ("public", "internal"). */
  visibility?: string;
  response_phase?: ConsoleAgent["response_phase"];
};

/** Status words and tone for an identity: the repair hold and health problems
 * outrank the lifecycle state; a busy agent whose health the team cannot
 * judge reads Working, never Unknown. */
export function agentStatusChip(
  stateLabel: string,
  health: string | null | undefined,
  working = false,
): { label: string; tone: Tone } {
  const state = stateLabel.trim().toLowerCase();
  if (state === "needs repair") return { label: "Needs repair", tone: "warn" };
  if (state === "retired" || state === "retiring" || state === "stopped") {
    return { label: humanizeEnum(state), tone: "neutral" };
  }
  if (state === "broken") return { label: "Broken", tone: "crit" };
  if (health === "wedged") return { label: "Stalled", tone: "crit" };
  if (health === "degraded") return { label: "Degraded", tone: "warn" };
  if (working) return { label: "Working", tone: "live" };
  if (health === "healthy") return { label: "Healthy", tone: "ok" };
  if (health === "unknown") return { label: "Not observed", tone: "locked" };
  return { label: humanizeEnum(state || "unknown"), tone: statusTone(state) };
}

export function reachabilityWords(addressability: string | null | undefined): string {
  if (addressability === "addressable") return "Can receive messages";
  if (addressability === "internal_only") return "Internal only";
  return humanizeEnum(addressability || "");
}

function CommandBlock({ command, testId, label }: { command: string; testId: string; label: string }) {
  return (
    <div className="agent-details__command">
      <code data-testid={testId}>{command}</code>
      <CopyButton text={command} label={`Copy the ${label} command`} className="agent-details__command-copy" />
    </div>
  );
}

/** A micro-label part of a section: never a heading (the panel's outline is
 * h2 name, h3 Overview, Latest output, Schedules). */
function Part({ label, action, testId, children }: { label: string; action?: React.ReactNode; testId?: string; children?: React.ReactNode }) {
  return (
    <div className="agent-details__part" data-testid={testId}>
      <div className="agent-details__part-head">
        <span className="agent-details__part-label">{label}</span>
        {action ? <span className="agent-details__part-action">{action}</span> : null}
      </div>
      {children}
    </div>
  );
}

function peerKey(peer: unknown): string {
  if (typeof peer === "string") return peer.trim();
  if (peer && typeof peer === "object") {
    const record = peer as Record<string, unknown>;
    const value = record.identity ?? record.member_id ?? record.id;
    if (typeof value === "string") return value.trim();
  }
  return "";
}

function findAgent(agents: readonly ConsoleAgent[] | undefined, key: string): ConsoleAgent | undefined {
  const bare = key.split("/").pop() || key;
  return agents?.find((agent) =>
    agent.identity === key || agent.member_id === key || agent.agent_id === key
    || agent.identity === bare || agent.member_id === bare);
}

/** A finished, failed or open run in words: "Finished in 4.2s · 3 tool calls". */
export function runWords(run: LedgerRunSpan, nowMs: number): { words: string; tone: Tone } {
  const tools = run.toolCalls ? ` · ${countWords(run.toolCalls, "tool call")}` : "";
  if (run.endMs === undefined) {
    return { words: `Running${run.startMs ? ` for ${shortSpan(nowMs - run.startMs)}` : ""}${tools}`, tone: "live" };
  }
  const took = run.startMs !== undefined ? shortSpan(run.endMs - run.startMs) : "";
  if (run.outcome === "failed") return { words: `Failed${took ? ` after ${took}` : ""}${tools}`, tone: "crit" };
  if (run.outcome === "cancelled") return { words: `Stopped${took ? ` after ${took}` : ""}${tools}`, tone: "neutral" };
  return { words: `Finished${took ? ` in ${took}` : ""}${tools}`, tone: "ok" };
}

/** The tool names of the inspect read's recent tool calls, newest first. */
function recentToolNames(inspect: IdentityInspectViewState | undefined | null): string[] {
  const names: string[] = [];
  for (const call of inspect?.recent_tool_calls || []) {
    const record = call as unknown as Record<string, unknown>;
    const name = record?.name ?? record?.tool_name;
    if (typeof name === "string" && name.trim() && !names.includes(name.trim())) names.push(name.trim());
  }
  return names.slice(0, 6);
}

/**
 * Agent details (console v2 spec B9): one agent's page. Who it is (avatar,
 * name, role, status), what it is doing now and whether it is healthy, a
 * glance at its day, its activity, whom it is connected to (an ego graph),
 * its latest output and recent turns, its schedules. Respawn, Reset and
 * Retire sit in the overflow menu; Reset and Retire ask first. The
 * identity, session, runtime and generation stay in Technical details.
 */
export function AgentDetailsPanel({
  identity,
  inspect,
  agent,
  stateLabel,
  actions,
  onOpenChat,
  openChatLabel = "Open chat",
  onRefresh,
  agents,
  work,
  now,
  children,
}: {
  identity: string;
  /** The inspect read; undefined while it loads, null when it could not be read. */
  inspect: ConsoleInspectView | IdentityInspectViewState | undefined | null;
  agent: ConsoleAgent | undefined;
  /** The state label the console shows (the repair hold reads "needs repair"). */
  stateLabel: string | null;
  actions: { respawn?: AgentDetailsAction; reset?: AgentDetailsAction; retire?: AgentDetailsAction };
  onOpenChat?: () => void;
  openChatLabel?: string;
  /** Reads the agent's details again. */
  onRefresh?: () => void;
  /** The roster: names, roles and live state of the agent's peers. */
  agents?: ConsoleAgent[];
  /** The work items it owns or has claimed, when the console has read them. */
  work?: AgentWorkItem[];
  /** A fixed clock (tests, the gallery). */
  now?: () => number;
  /** The schedules section, when offered. */
  children?: React.ReactNode;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  const navigation = useConsoleNavigation();
  const ledger = useActivityLedger({ tickMs: 30_000 });
  const nowMs = now ? now() : ledger.nowMs;
  const [showIds, setShowIds] = useShowTechnicalIds("agent-details");
  const [confirming, setConfirming] = React.useState<ConfirmedLifecycleAction | null>(null);
  const view = inspect as ConsoleInspectView | undefined | null;
  // Named the way the sidebar names the agent.
  const name = agent
    ? agentDisplayTitle({ ...agent, label: agent.label || view?.display_name || "" })
    : view?.display_name || agentDisplayName(identity, directory) || identity;
  const role = view?.role || agent?.role;
  // The gateway's inspect payload carries no progress; the roster row does.
  const progress = view?.progress ?? agent?.progress;
  const health = progress?.health;
  const activity = agentActivity(ledger, {
    identity: agent?.identity || identity,
    member_id: agent?.member_id || identity,
    agent_id: agent?.agent_id || identity,
    last_activity_ms: view?.last_activity_ms ?? agent?.last_activity_ms,
    progress,
  }, nowMs);
  const statusFields = {
    state: agent?.state ?? stateLabel ?? undefined,
    response_phase: view?.response_phase !== undefined ? view.response_phase : agent?.response_phase,
    progress,
    labels: agent?.labels ?? view?.labels,
    unobserved: view?.unobserved ?? agent?.unobserved,
  };
  const working = isAgentWorking(statusFields, { openRun: activity.openRun });
  const fullHealth: AgentHealth = agentHealth(statusFields, { openRun: activity.openRun });
  const status = stateLabel !== null ? agentStatusChip(stateLabel, health, working) : null;
  const nowLine = agent
    ? agentNowLine({ ...agent, progress, response_phase: statusFields.response_phase }, activity, nowMs)
    : working
      ? { text: "Working", live: true }
      : { text: fullHealth.word, live: false };
  const repair = view?.session_repair;
  const peers = React.useMemo(() => {
    const keys = new Set<string>();
    for (const peer of view?.topology_peers || []) if (peer) keys.add(peer);
    for (const peer of agent?.wired_to || []) {
      const key = peerKey(peer);
      if (key) keys.add(key);
    }
    return [...keys];
  }, [view?.topology_peers, agent?.wired_to]);
  const lastActivity = view?.last_activity_ms && view.last_activity_ms > 0
    ? view.last_activity_ms
    : activity.lastActivityMs;
  const roleGroup = [humanizeEnum(role || ""), agent?.group ? humanizeEnum(agent.group) : ""].filter(Boolean).join(" · ");

  const menuItems: MenuEntry[] = [];
  if (actions.respawn) {
    menuItems.push({ kind: "item", id: "respawn", label: actions.respawn.label, onSelect: actions.respawn.onClick, testId: `inspect-action:${identity}:respawn` });
  }
  if (actions.reset) {
    menuItems.push({ kind: "item", id: "reset", label: `${actions.reset.label}…`, textValue: actions.reset.label, onSelect: () => setConfirming("reset"), testId: `inspect-action:${identity}:reset` });
  }
  if (actions.retire) {
    menuItems.push({ kind: "item", id: "retire", label: `${actions.retire.label}…`, textValue: actions.retire.label, destructive: true, onSelect: () => setConfirming("retire"), testId: `inspect-action:${identity}:retire` });
  }
  if (menuItems.length) menuItems.push({ kind: "separator", id: "lifecycle" });
  menuItems.push({ kind: "checkbox", id: "technical-ids", label: "Show technical IDs", checked: showIds, onChange: setShowIds, testId: "inspect-menu:technical-ids" });
  if (onRefresh) menuItems.push({ kind: "item", id: "refresh", label: "Refresh now", glyph: "refresh", onSelect: onRefresh, testId: "inspect-menu:refresh" });

  const summary: Array<SummaryFact | null> = [
    roleGroup ? { key: "role", text: roleGroup, announce: false } : null,
    view ? { key: "reach", text: reachabilityWords(view.addressability), announce: false } : null,
    lastActivity ? { key: "last", text: <>last active <RelativeTime value={lastActivity} /></>, words: "last active", announce: false } : null,
  ];

  const turnsToday = activity.today.turnsStarted;
  const runs = React.useMemo(
    () => ledger.runSpans(activity.key, { sinceMs: nowMs - DAY_MS, untilMs: nowMs }).slice(-6).reverse(),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ledger.version, activity.key, Math.floor(nowMs / 30_000)],
  );
  const toolNames = recentToolNames(view);
  const reachable = typeof view?.peer_reachable_count === "number" ? view.peer_reachable_count : null;

  const confirmAction = confirming ? actions[confirming] : undefined;
  // The day's bars take the part's width (200 to 640 px), so they fit a narrow pane.
  const [barsRef, barsMeasured] = useMeasuredWidth<HTMLDivElement>();
  const barsWidth = barsMeasured > 0 ? Math.max(200, Math.min(640, barsMeasured)) : 480;

  return (
    <PanelShell className="agent-details" testId={`inspect-panel:${identity}`} showTechnicalIds={showIds}>
      <PanelHeader
        className="agent-details__head"
        lead={<AgentAvatar name={name} size={36} live={working} />}
        title={<span title={identity}>{name}</span>}
        summary={summary}
        summaryTestId={`inspect-summary:${identity}`}
        actions={
          <>
            {status ? <StatusPill tone={status.tone} testId={`inspect-status:${identity}`}>{status.label}</StatusPill> : null}
            {onOpenChat ? <Button variant="primary" onClick={onOpenChat} data-testid={`inspect-chat:${identity}`}>{openChatLabel}</Button> : null}
          </>
        }
        menu={{ label: `More actions for ${name}`, items: menuItems, triggerTestId: `inspect-menu:${identity}` }}
      />
      {confirming && confirmAction ? (
        <div className="agent-details__confirm">
          <LifecycleConfirm
            action={confirming}
            name={name}
            actionLabel={confirmAction.label}
            onConfirm={() => {
              setConfirming(null);
              confirmAction.onClick();
            }}
            onCancel={() => setConfirming(null)}
            testId={`inspect-confirm:${identity}:${confirming}`}
            confirmTestId={`inspect-confirm-yes:${identity}:${confirming}`}
            cancelTestId={`inspect-confirm-cancel:${identity}:${confirming}`}
          />
        </div>
      ) : null}
      {view ? (
        <GlanceStrip ariaLabel={`${name} today`} testId={`inspect-glance:${identity}`}>
          <FigureTile
            label="Turns today"
            value={turnsToday}
            live={working}
            trend={activity.hourlyTurns}
            trendLabel={seriesWords(activity.hourlyTurns, activity.endMs, "turn")}
            trendLive={working}
            testId="inspect-glance:turns"
          />
          <FigureTile label="Messages" value={activity.today.messagesSent} foot="sent today" testId="inspect-glance:messages" />
          <FigureTile label="Tool calls" value={activity.today.toolCalls} foot="today" testId="inspect-glance:tools" />
          <FigureTile
            label="Failures"
            value={activity.today.failures}
            tone={activity.today.failures ? "crit" : undefined}
            foot={activity.today.failures ? "today" : "none today"}
            testId="inspect-glance:failures"
          />
        </GlanceStrip>
      ) : null}
      <PanelBody className="agent-details__body">
        {view === undefined ? (
          <LoadingState label="Loading agent details" rows={4} />
        ) : view === null ? (
          <EmptyState
            kind="error"
            title="Couldn't load this agent's details"
            body="The gateway's answer could not be read. Try again in a moment."
            action={onRefresh ? <Button size="sm" onClick={onRefresh}>Retry</Button> : undefined}
          />
        ) : (
          <>
            {repair ? (
              <div className="agent-details__repair" data-testid={`inspect-session-repair:${identity}`}>
                <Notice tone="warn">
                  <p>
                    The saved session is intact but refused until it is repaired. Run the diagnose
                    command, then the repair command, then reload the member
                    (<code>meta/reload_member</code>).
                  </p>
                  <CommandBlock command={repair.diagnose_command} testId="inspect-session-repair-diagnose" label="diagnose" />
                  <CommandBlock command={repair.apply_command} testId="inspect-session-repair-apply" label="repair" />
                </Notice>
              </div>
            ) : null}
            <PanelSection label="Overview" padded>
              <Part label="Now" testId={`inspect-now:${identity}`}>
                <p className="agent-details__now" data-live={nowLine.live ? "true" : undefined}>
                  {nowLine.live ? <StatusMark tone="live" /> : null}
                  {nowLine.text}
                </p>
              </Part>
              <KeyValueList
                items={[
                  {
                    label: "Status",
                    value: (
                      <span data-testid={`inspect-state:${identity}`}>
                        {status ? status.label : humanizeEnum(stateLabel || "")}
                      </span>
                    ),
                  },
                  role ? { label: "Role", value: humanizeEnum(role) } : null,
                  { label: "Reachability", value: reachabilityWords(view.addressability) },
                  lastActivity ? { label: "Last activity", value: <RelativeTime value={lastActivity} /> } : null,
                  view.unobserved ? { label: "Not observed", value: humanizeEnum(String(view.unobserved.reason)) } : null,
                ]}
              />
              <Part label="Activity · 24h" action={`${countWords(turnsToday, "turn")} today`} testId={`inspect-activity:${identity}`}>
                <div ref={barsRef} className="agent-details__bars-box" />
                <ActivityBars
                  values={activity.hourly}
                  end={activity.endMs}
                  width={barsWidth}
                  height={48}
                  live={working}
                  unit="events"
                  label={seriesWords(activity.hourly, activity.endMs, "event")}
                  className="agent-details__bars"
                />
              </Part>
              <Part
                label="Connections"
                action={peers.length
                  ? reachable !== null
                    ? `${reachable} of ${plural(peers.length, "peer")} reachable`
                    : plural(peers.length, "peer")
                  : undefined}
                testId={`inspect-connections:${identity}`}
              >
                {peers.length ? (
                  <>
                    <EgoGraph
                      identity={identity}
                      name={name}
                      working={working}
                      peers={peers}
                      agents={agents}
                      ledgerWorking={(key) => ledger.hasOpenRun(key)}
                      onOpenPeer={navigation?.openAgent ? (peer) => navigation.openAgent?.(peer, "details") : undefined}
                    />
                    <span className="cc-chips agent-details__peers" aria-label="Connected to">
                      {peers.map((peer) => (
                        <span className="agent-details__peer" key={peer}>
                          <AgentName id={peer} avatar={18} link />
                        </span>
                      ))}
                    </span>
                  </>
                ) : (
                  <p className="agent-details__muted">Nobody yet. Wire this agent to others in Topology.</p>
                )}
              </Part>
            </PanelSection>
            {view.output_preview ? (
              <PanelSection
                label="Latest output"
                action={view.is_final === false ? <span className="agent-details__muted">still writing</span> : undefined}
                padded
              >
                <blockquote className="agent-details__preview">{view.output_preview}</blockquote>
              </PanelSection>
            ) : null}
            <PanelSection label="Recent turns" headerAs="div" padded testId="inspect-recent-turns">
              {runs.length ? (
                <ol className="agent-details__turns">
                  {runs.map((run) => {
                    const words = runWords(run, nowMs);
                    const at = run.startMs ?? run.endMs;
                    return (
                      <li key={run.key} className="agent-details__turn" data-tone={words.tone}>
                        <span className="agent-details__turn-time">{at ? formatClockTime(at) : "--:--"}</span>
                        <StatusMark tone={words.tone} />
                        <span className="agent-details__turn-words">{words.words}</span>
                      </li>
                    );
                  })}
                </ol>
              ) : (
                <p className="agent-details__muted">No turns in the last 24 hours that this console saw.</p>
              )}
              {toolNames.length ? (
                <p className="agent-details__tools">
                  <span className="agent-details__part-label">Recent tools</span>
                  {toolNames.map((tool) => <code key={tool} className="agent-details__tool">{tool}</code>)}
                </p>
              ) : null}
            </PanelSection>
            {work && work.length ? (
              <PanelSection label="Work" count={work.length} headerAs="div" padded testId={`inspect-work:${identity}`}>
                <div className="agent-details__work">
                  {work.map((item) => {
                    const live = item.status === "in_progress";
                    return (
                      <EntityRow
                        key={item.id}
                        kind="task"
                        testId={`inspect-work-item:${item.id}`}
                        className="agent-details__work-item"
                        mark={<StatusSquare status={item.status} live={live} />}
                        title={<EntityLink kind="work-item" id={item.id} title={item.id}>{item.title}</EntityLink>}
                        subtitle={item.claimedAt ? <>claimed <RelativeTime value={item.claimedAt} /></> : undefined}
                        status={<StatusPill tone={live ? "live" : statusTone(item.status)} variant="quiet">{humanizeEnum(item.status)}</StatusPill>}
                        live={live}
                      />
                    );
                  })}
                </div>
              </PanelSection>
            ) : null}
            <div className="agent-details__tech">
              <TechnicalDetails
                items={[
                  { label: "Identity", value: identity },
                  { label: "Session", value: view.continuity?.session_id },
                  { label: "Runtime", value: view.continuity?.agent_runtime_id },
                  view.runtime_key ? { label: "Runtime key", value: view.runtime_key } : null,
                  view.visibility ? { label: "Visibility", value: view.visibility, plain: true } : null,
                  typeof view.continuity?.generation === "number"
                    ? { label: "Generation", value: view.continuity.generation }
                    : null,
                ]}
              />
            </div>
          </>
        )}
        {children}
      </PanelBody>
    </PanelShell>
  );
}

/** The agent at the centre, its peers around it (spec B9 CONNECTIONS). */
function EgoGraph({
  identity,
  name,
  working,
  peers,
  agents,
  ledgerWorking,
  onOpenPeer,
}: {
  identity: string;
  name: string;
  working: boolean;
  peers: string[];
  agents?: ConsoleAgent[];
  ledgerWorking: (key: string) => boolean;
  onOpenPeer?: (identity: string) => void;
}) {
  const directory = useAgentDirectory();
  const { nodes, edges } = React.useMemo(() => {
    const centre: VizNode = {
      id: identity,
      kind: "agent",
      label: name,
      live: working,
      tone: working ? "live" : undefined,
      size: "lg",
    };
    const peerNodes: VizNode[] = peers.map((peer) => {
      const peerAgent = findAgent(agents, peer);
      const label = peerAgent ? agentDisplayTitle(peerAgent) : agentDisplayName(peer, directory) || peer;
      const roleWord = (peerAgent?.role || "").toLowerCase();
      const helper = Boolean(peerAgent?.side_chat_of || peerAgent?.labels?.side_chat_of || peerAgent?.labels?.spawned_by);
      const live = peerAgent
        ? isAgentWorking(peerAgent, { openRun: ledgerWorking(peerAgent.identity || peerAgent.member_id) })
        : false;
      return {
        id: peer,
        kind: !peerAgent ? "unknown" : helper ? "helper" : roleWord.includes("coord") || roleWord.includes("lead") ? "coordinator" : "agent",
        label,
        sublabel: peerAgent?.role ? humanizeEnum(peerAgent.role) : undefined,
        live,
        tone: live ? "live" : undefined,
        title: peer,
      };
    });
    const peerEdges: VizEdge[] = peers.map((peer) => ({ id: `${identity}->${peer}`, from: identity, to: peer, relation: "wired" }));
    return { nodes: [centre, ...peerNodes], edges: peerEdges };
  }, [identity, name, working, peers, agents, directory, ledgerWorking]);
  return (
    <div className="agent-details__ego">
      <GraphView
        nodes={nodes}
        edges={edges}
        layout="radial"
        center={identity}
        height={240}
        ariaLabel={`${name} and ${plural(peers.length, "connection")}`}
        legend="off"
        minimap="off"
        controls={false}
        labels="all"
        // Inside the scrolling details body: plain scrolling moves the page,
        // a pinch or Ctrl/Cmd + wheel zooms the graph.
        wheelZoom="pinch"
        testIdPrefix="inspect-ego"
        selection={[]}
        onSelect={(ids) => {
          const peer = ids.find((id) => id !== identity);
          if (peer) onOpenPeer?.(peer);
        }}
        onOpen={(id) => {
          if (id !== identity) onOpenPeer?.(id);
        }}
      />
    </div>
  );
}

/** The hour labels of the last day, oldest first (for charts' table twins). */
export function lastDayHours(endMs: number): string[] {
  return Array.from({ length: 24 }, (_, index) => hourLabel(endMs - (24 - index) * HOUR_MS));
}
