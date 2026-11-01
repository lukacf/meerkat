// The Topology drawer (design spec B1, L2): the selected agent (now,
// activity, connections, memory, actions, technical details), the selected
// edge (its relation, both ends and its numbers), a channel or a memory
// store. Connect to... turns the drawer into the connection editor for that
// agent.

import React from "react";

import {
  topologyAffordanceFor,
  topologyMutationIntent,
  type TopologyEdgeRef,
  type TopologyManagementState,
  type TopologyMutationIntent,
  type TopologyMutationKind,
} from "@console-core";

import {
  ActivityBars,
  AgentAvatar,
  Button,
  DetailDrawer,
  DrawerSection,
  StatusPill,
  formatDuration,
  formatRelativeTime,
  humanizeEnum,
  plural,
  type TechnicalDetail,
} from "../panel-kit";
import { KitGlyph } from "../panel-kit/glyphs";
import type { TopoAgent, TopoEdge, TopoGraph, TopoSource } from "./data";
import type { ConsoleTopologyAgentStatus, ConsoleTopologyMemoryScope, ConsoleTopologyStore } from "./types";
import {
  TOPOLOGY_WINDOWS,
  topologyLineageWord,
  topologyStatusPresentation,
  type TopologyWindowId,
} from "./view-model";

/** What the agent is doing now, from its owner-reported phase and its open tool. */
export interface TopologyNowLine {
  text: string;
  detail?: string;
  live: boolean;
}

/** The 24 h activity the drawer draws: turns per bin, oldest first. */
export interface TopologyAgentActivity {
  values: number[];
  total: number;
  /** "turns" by default. */
  unit?: string;
}

export interface TopologyDrawerTarget {
  kind: "agent" | "edge" | "source" | "store";
  id: string;
}

export interface TopologyDrawerProps {
  graph: TopoGraph;
  target: TopologyDrawerTarget;
  statusOf: (id: string) => ConsoleTopologyAgentStatus | undefined;
  window: TopologyWindowId;
  teamName?: string;
  /** The agent's section word ("Coordinators"). */
  sectionOf: (agent: TopoAgent) => string;
  nowOf: (agent: TopoAgent) => TopologyNowLine | null;
  activityOf: (agent: TopoAgent) => TopologyAgentActivity | null;
  memoryOf: (agent: TopoAgent) => ConsoleTopologyMemoryScope | null;
  /** When the agent's memory was last given to a turn (a memory read), if seen. */
  memoryGivenOf?: (agent: TopoAgent) => number | null;
  management?: TopologyManagementState | null;
  onRequestMutation?: (intent: TopologyMutationIntent) => void | Promise<void>;
  /** The agent can be connected from here: Connect to... shows. */
  canConnect: (agent: TopoAgent) => boolean;
  onConnectTo?: (agent: TopoAgent) => void;
  onOpenChat?: (agent: TopoAgent) => void;
  onOpenDetails?: (agent: TopoAgent) => void;
  onOpenMemory?: (agent: TopoAgent | null) => void;
  onSelect: (target: TopologyDrawerTarget) => void;
  onClose: () => void;
  /** The connection editor for the agent, when Connect to... is open. */
  editor?: React.ReactNode;
  now: number;
}

const PHASE_WORDS: Record<string, string> = {
  generating: "Writing a reply",
  "tool-executing": "Running a tool",
  waiting: "Waiting for a reply",
};

/** The agent's now line from its phase, its open tool and its open run. */
export function topologyNowLine(
  agent: TopoAgent,
  status: ConsoleTopologyAgentStatus | undefined,
  openTool: { name: string; startedAtMs: number } | null,
  now: number,
): TopologyNowLine {
  if (status?.retired) return { text: "Retired", live: false };
  if (status?.working) {
    const phase = agent.responsePhase ? PHASE_WORDS[agent.responsePhase] : undefined;
    if (openTool) {
      return {
        text: phase || "Running a tool",
        detail: [openTool.name, formatDuration(now - openTool.startedAtMs)].filter(Boolean).join(" · "),
        live: true,
      };
    }
    return { text: phase || "Working on a turn", live: true };
  }
  if (status?.health === "crit") return { text: status.healthWord === "Broken" ? "Broken" : "Stuck: its run makes no progress", live: false };
  // When it last moved: the runtime's own progress clock.
  const lastMs = agent.agent?.progress?.last_progress_at_ms;
  const since = typeof lastMs === "number" && lastMs > 0 ? formatRelativeTime(lastMs, now) : "";
  return { text: "Idle", ...(since ? { detail: `last active ${since}` } : {}), live: false };
}

function label(graph: TopoGraph, id: string): string {
  return graph.byId.get(id)?.label || graph.sources.find((source) => source.id === id)?.label || id;
}

interface ConnectionRow {
  key: string;
  peer: string;
  peerKind: "agent" | "source";
  words: string[];
  edges: TopoEdge[];
  action?: { action: TopologyMutationKind; label: string; edge: TopologyEdgeRef };
}

function connectionRows(
  graph: TopoGraph,
  agent: TopoAgent,
  window: TopologyWindowId,
  management: TopologyManagementState | null | undefined,
  canMutate: boolean,
): ConnectionRow[] {
  const rows = new Map<string, ConnectionRow>();
  const windowWords = TOPOLOGY_WINDOWS[window];
  for (const edge of graph.edges) {
    if (edge.from !== agent.id && edge.to !== agent.id) continue;
    const peer = edge.from === agent.id ? edge.to : edge.from;
    const row = rows.get(peer) || {
      key: peer,
      peer,
      peerKind: edge.relation === "route" ? "source" as const : "agent" as const,
      words: [],
      edges: [],
    };
    row.edges.push(edge);
    switch (edge.relation) {
      case "wired":
      case "cross":
        if (edge.state === "pending") row.words.push(edge.pendingAction === "disconnect" ? "disconnecting" : "connecting");
        else if (edge.degraded) row.words.push("declared, not wired");
        else if (edge.suppressed) row.words.push("disconnected by an operator");
        else row.words.push(edge.relation === "cross" ? "other team" : "wired");
        break;
      case "traffic": {
        const sent = edge.from === agent.id ? edge.counts?.forward ?? 0 : edge.counts?.backward ?? 0;
        const received = edge.from === agent.id ? edge.counts?.backward ?? 0 : edge.counts?.forward ?? 0;
        row.words.push(`${plural(sent + received, "message")} ${windowWords.phrase}`);
        break;
      }
      case "route":
        row.words.push(`route · ${edge.channel || "channel"}`);
        if (edge.failedDeliveries) row.words.push(`${edge.failedDeliveries} failed`);
        break;
      case "lineage":
        row.words.push(edge.to === agent.id
          ? `started this ${topologyLineageWord(edge.lineage)}`
          : topologyLineageWord(edge.lineage));
        break;
    }
    rows.set(peer, row);
  }
  for (const row of rows.values()) {
    if (!management || !canMutate) continue;
    const comms = row.edges.find((edge) => (edge.relation === "wired" || edge.relation === "cross") && edge.endpoints);
    if (!comms?.endpoints || comms.state === "pending") continue;
    const affordance = topologyAffordanceFor(management, comms.endpoints);
    const action: TopologyMutationKind | null = comms.state === "declared"
      ? (affordance?.preferredAction === "reconnect" || comms.suppressed ? "reconnect" : "connect")
      : "disconnect";
    if (action && topologyMutationIntent(management, action, comms.endpoints, "graph")) {
      row.action = {
        action,
        label: action === "disconnect" ? "Disconnect" : action === "reconnect" ? "Reconnect" : "Connect",
        edge: comms.endpoints,
      };
    }
  }
  return Array.from(rows.values()).sort((left, right) => {
    const kind = (row: ConnectionRow) => row.edges.some((edge) => edge.relation === "wired" || edge.relation === "cross") ? 0 : row.peerKind === "source" ? 2 : 1;
    return kind(left) - kind(right) || label(graph, left.peer).localeCompare(label(graph, right.peer));
  });
}

function AgentDrawer(props: TopologyDrawerProps & { agent: TopoAgent }): React.JSX.Element {
  const { agent, graph, management, onRequestMutation, now } = props;
  const status = props.statusOf(agent.id);
  const presentation = topologyStatusPresentation(status || { working: false });
  const nowLine = props.nowOf(agent);
  const activity = props.activityOf(agent);
  const memory = props.memoryOf(agent);
  const memoryGivenAt = props.memoryGivenOf?.(agent) ?? null;
  const rows = connectionRows(graph, agent, props.window, management, Boolean(onRequestMutation));
  const role = humanizeEnum(agent.role);
  const subtitle = [
    agent.kind === "helper" && agent.parent
      ? `${humanizeEnum(topologyLineageWord(agent.lineage))} of ${label(graph, agent.parent)}`
      : role,
    agent.kind === "external" ? agent.authority || "Another team" : props.teamName || props.sectionOf(agent),
  ].filter(Boolean).join(" · ");
  const technical: TechnicalDetail[] = [
    { label: "Identity", value: agent.ref.identity || agent.id },
    ...(agent.ref.id && agent.ref.id !== (agent.ref.identity || agent.id) ? [{ label: "Endpoint", value: agent.ref.id }] : []),
    ...(agent.authority ? [{ label: "Authority", value: agent.authority }] : []),
    ...(agent.memberId && agent.memberId !== agent.id ? [{ label: "Member", value: agent.memberId }] : []),
    ...(agent.agent?.session_id ? [{ label: "Session", value: agent.agent.session_id }] : []),
  ];
  return (
    <DetailDrawer
      testId="topology-drawer"
      title={agent.label}
      mark={<AgentAvatar name={agent.label} size={28} live={status?.working} />}
      status={<StatusPill tone={presentation.tone}>{presentation.word}</StatusPill>}
      subtitle={subtitle}
      onClose={props.onClose}
      technical={props.editor ? undefined : technical}
      actions={(
        <>
          {props.onOpenChat && agent.kind !== "external" ? (
            <Button variant="primary" size="sm" onClick={() => props.onOpenChat?.(agent)} data-testid="topology-drawer-open-chat">
              <KitGlyph name="chat" />
              Open chat
            </Button>
          ) : null}
          {props.onOpenDetails && agent.kind !== "external" ? (
            <Button size="sm" onClick={() => props.onOpenDetails?.(agent)} data-testid="topology-drawer-details">Details</Button>
          ) : null}
          {props.onConnectTo && props.canConnect(agent) ? (
            <Button size="sm" onClick={() => props.onConnectTo?.(agent)} data-testid="topology-drawer-connect">Connect to...</Button>
          ) : null}
        </>
      )}
    >
      {props.editor ? (
        <div className="topo-drawer__editor">{props.editor}</div>
      ) : (
        <>
          {nowLine ? (
            <DrawerSection label="Now">
              <p className={`topo-drawer__now${nowLine.live ? " is-live" : ""}`}>
                {nowLine.live ? <span className="topo-drawer__now-mark" aria-hidden="true" /> : null}
                <span>{nowLine.text}</span>
              </p>
              {nowLine.detail ? <p className="topo-drawer__now-detail">{nowLine.detail}</p> : null}
            </DrawerSection>
          ) : null}
          {activity ? (
            <DrawerSection label="Activity · 24h" action={<span className="topo-drawer__aside">{plural(activity.total, activity.unit === "messages" ? "message" : "turn")}</span>}>
              <ActivityBars
                values={activity.values}
                end={now}
                live={Boolean(status?.working)}
                unit="turns"
                label={`${agent.label}: ${plural(activity.total, "turn")} in the last 24 hours`}
              />
            </DrawerSection>
          ) : null}
          <DrawerSection label="Connections" count={rows.length}>
            {rows.length === 0 ? (
              <p className="topo-drawer__empty">Not wired to anyone, no route feeds it and it has sent no messages {TOPOLOGY_WINDOWS[props.window].phrase}.</p>
            ) : (
              <ul className="topo-drawer__connections" data-testid="topology-drawer-connections">
                {rows.map((row) => (
                  <li key={row.key} className="topo-drawer__connection" data-testid={`topology-drawer-connection:${row.peer}`}>
                    {row.peerKind === "source" ? (
                      <button type="button" className="topo-drawer__peer is-source" onClick={() => props.onSelect({ kind: "source", id: row.peer })}>
                        <span className="topo-drawer__source-mark" aria-hidden="true" />
                        {label(graph, row.peer)}
                      </button>
                    ) : (
                      <button type="button" className="topo-drawer__peer" onClick={() => props.onSelect({ kind: "agent", id: row.peer })}>
                        <AgentAvatar name={label(graph, row.peer)} size={18} />
                        <span className="topo-drawer__peer-name">{label(graph, row.peer)}</span>
                      </button>
                    )}
                    <span className="topo-drawer__words">{row.words.join(" · ")}</span>
                    {row.action ? (
                      <Button
                        size="sm"
                        variant="ghost"
                        className="topo-drawer__row-action cc-reveal"
                        aria-label={`${row.action.label} ${label(graph, row.peer)} ${row.action.action === "disconnect" ? "from" : "to"} ${agent.label}`}
                        onClick={() => {
                          if (!management) return;
                          const intent = topologyMutationIntent(management, row.action!.action, row.action!.edge, "graph");
                          if (intent) void onRequestMutation?.(intent);
                        }}
                      >
                        {row.action.label}
                      </Button>
                    ) : null}
                  </li>
                ))}
              </ul>
            )}
          </DrawerSection>
          {memory ? (
            <DrawerSection
              label="Memory"
              action={props.onOpenMemory ? (
                <button type="button" className="topo-drawer__link" onClick={() => props.onOpenMemory?.(agent)}>Open in Memory</button>
              ) : undefined}
            >
              <p className="topo-drawer__fact">
                {plural(memory.records, "record")}
                {memory.quarantined ? ` · ${memory.quarantined} quarantined` : ""}
                {memoryGivenAt ? ` · last given ${formatRelativeTime(memoryGivenAt, props.now)}` : ""}
              </p>
            </DrawerSection>
          ) : null}
        </>
      )}
    </DetailDrawer>
  );
}

const RELATION_TITLES: Record<TopoEdge["relation"], string> = {
  wired: "Wired",
  traffic: "Messages",
  route: "Route",
  lineage: "Helper",
  cross: "Other team",
};

function EdgeDrawer(props: TopologyDrawerProps & { edge: TopoEdge }): React.JSX.Element {
  const { edge, graph, management, onRequestMutation } = props;
  const windowWords = TOPOLOGY_WINDOWS[props.window];
  const facts: string[] = [];
  if (edge.relation === "traffic") {
    facts.push(`${label(graph, edge.from)} to ${label(graph, edge.to)}: ${plural(edge.counts?.forward ?? edge.weight ?? 0, "message")}`);
    if (edge.counts?.backward) facts.push(`${label(graph, edge.to)} to ${label(graph, edge.from)}: ${plural(edge.counts.backward, "message")}`);
    if (edge.lastAtMs) facts.push(`Last one ${formatRelativeTime(edge.lastAtMs, props.now)}`);
    if (edge.wired) facts.push("The pair is wired");
  } else if (edge.relation === "route") {
    for (const route of edge.routes || []) facts.push(`${route.key}${route.sink ? ` (${route.sink})` : ""}`);
    if (edge.sentDeliveries || edge.failedDeliveries) {
      facts.push(`${edge.sentDeliveries || 0} sent, ${edge.failedDeliveries || 0} failed`);
    }
  } else if (edge.relation === "lineage") {
    facts.push(`${label(graph, edge.from)} started ${label(graph, edge.to)} as a ${topologyLineageWord(edge.lineage)}`);
  } else {
    if (edge.state === "pending") facts.push(edge.pendingAction === "disconnect" ? "Disconnecting now" : "Connecting now");
    else if (edge.degraded) facts.push("The team declares this pair, and it is not wired");
    else if (edge.suppressed) facts.push("Declared by the team, disconnected by an operator");
    else if (edge.conflict) facts.push("Still wired, though an operator asked to disconnect it");
    else facts.push(edge.operatorAdded ? "Wired by an operator" : "Wired by the team's definition");
    if (edge.relation === "cross") facts.push("The other end is in another team");
  }
  const comms = (edge.relation === "wired" || edge.relation === "cross") && edge.endpoints ? edge.endpoints : null;
  const action: TopologyMutationKind | null = comms && edge.state !== "pending"
    ? edge.state === "declared" ? (edge.suppressed ? "reconnect" : "connect") : "disconnect"
    : null;
  const intentAvailable = Boolean(management && comms && action && onRequestMutation
    && topologyMutationIntent(management, action, comms, "graph"));
  return (
    <DetailDrawer
      testId="topology-drawer"
      title={RELATION_TITLES[edge.relation]}
      subtitle={edge.relation === "traffic" ? windowWords.phrase.replace(/^in /, "In ") : undefined}
      onClose={props.onClose}
      actions={intentAvailable ? (
        <Button
          size="sm"
          onClick={() => {
            const intent = management && comms && action ? topologyMutationIntent(management, action, comms, "graph") : null;
            if (intent) void onRequestMutation?.(intent);
          }}
          data-testid="topology-drawer-edge-action"
        >
          {action === "disconnect" ? "Disconnect" : action === "reconnect" ? "Reconnect" : "Connect"}
        </Button>
      ) : undefined}
    >
      <DrawerSection label="Between">
        <ul className="topo-drawer__connections">
          {[edge.from, edge.to].map((end) => (
            <li key={end} className="topo-drawer__connection">
              <button
                type="button"
                className="topo-drawer__peer"
                onClick={() => props.onSelect({ kind: graph.byId.has(end) ? "agent" : "source", id: end })}
              >
                {graph.byId.has(end) ? <AgentAvatar name={label(graph, end)} size={18} /> : <span className="topo-drawer__source-mark" aria-hidden="true" />}
                {label(graph, end)}
              </button>
            </li>
          ))}
        </ul>
      </DrawerSection>
      <DrawerSection label="Details">
        <ul className="topo-drawer__facts">
          {facts.map((fact) => <li key={fact}>{fact}</li>)}
        </ul>
      </DrawerSection>
    </DetailDrawer>
  );
}

function SourceDrawer(props: TopologyDrawerProps & { source: TopoSource }): React.JSX.Element {
  const { source, graph } = props;
  return (
    <DetailDrawer
      testId="topology-drawer"
      title={source.label}
      subtitle={source.managed ? "Channel · every member's own route" : "Channel"}
      onClose={props.onClose}
    >
      <DrawerSection label="Feeds" count={source.recipients.length}>
        <ul className="topo-drawer__connections">
          {source.recipients.map((recipient) => (
            <li key={recipient} className="topo-drawer__connection">
              <button type="button" className="topo-drawer__peer" onClick={() => props.onSelect({ kind: "agent", id: recipient })}>
                <AgentAvatar name={label(graph, recipient)} size={18} />
                {label(graph, recipient)}
              </button>
              <span className="topo-drawer__words">
                {source.routes.filter((route) => graph.resolve(route.recipient) === recipient).map((route) => route.key).join(", ")}
              </span>
            </li>
          ))}
        </ul>
      </DrawerSection>
    </DetailDrawer>
  );
}

function StoreDrawer(props: TopologyDrawerProps & { store: ConsoleTopologyStore }): React.JSX.Element {
  const { store } = props;
  return (
    <DetailDrawer
      testId="topology-drawer"
      title={store.label}
      subtitle="Memory store"
      onClose={props.onClose}
      actions={props.onOpenMemory ? (
        <Button size="sm" onClick={() => props.onOpenMemory?.(null)}>Open in Memory</Button>
      ) : undefined}
    >
      <DrawerSection label="Records">
        <p className="topo-drawer__fact">{typeof store.records === "number" ? plural(store.records, "record") : "Not counted"}</p>
      </DrawerSection>
    </DetailDrawer>
  );
}

export function TopologyDrawer(props: TopologyDrawerProps): React.JSX.Element | null {
  const { target, graph } = props;
  if (target.kind === "agent") {
    const agent = graph.byId.get(target.id);
    return agent ? <AgentDrawer {...props} agent={agent} /> : null;
  }
  if (target.kind === "edge") {
    const edge = graph.edges.find((candidate) => candidate.id === target.id);
    return edge ? <EdgeDrawer {...props} edge={edge} /> : null;
  }
  if (target.kind === "source") {
    const source = graph.sources.find((candidate) => candidate.id === target.id);
    return source ? <SourceDrawer {...props} source={source} /> : null;
  }
  const store = graph.stores.find((candidate) => `store:${candidate.id}` === target.id);
  return store ? <StoreDrawer {...props} store={store} /> : null;
}
