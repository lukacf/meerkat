// Topology in the dev-only kit gallery (console v2 spec B26, B28): the panel
// on a fixture team that shows what a seeded runtime cannot produce today:
// a degraded and a stalled agent, another team's agent, a declared pair
// that is not wired, a connect in flight, a failing route and a message in
// flight. Never part of the production bundle: only the gallery's dev-time
// glob loads `*.gallery.tsx` files.

import React from "react";

import type { TopologyManagementState } from "@console-core";

import { TopologyPanel } from "./topology-panel";
import type {
  ConsoleAgent,
  ConsoleFrame,
  ConsoleTopologyLink,
  ConsoleTopologyNode,
  ConsoleTopologyRoute,
} from "./types";

const AUTHORITY = "operations";
const key = (identity: string, authority = AUTHORITY) =>
  `mk1|${encodeURIComponent(authority)}|${encodeURIComponent(identity)}`;

const TEAM: Array<{ identity: string; label: string; group: string; role: string }> = [
  { identity: "router", label: "Router", group: "Coordinators", role: "coordinator" },
  { identity: "triage", label: "Triage", group: "Coordinators", role: "coordinator" },
  { identity: "billing", label: "Billing", group: "Domains", role: "specialist" },
  { identity: "delivery", label: "Delivery", group: "Domains", role: "specialist" },
  { identity: "support", label: "Support", group: "Domains", role: "specialist" },
  { identity: "gate", label: "Gate", group: "Internal", role: "gate" },
  { identity: "health-monitor", label: "Health Monitor", group: "Internal", role: "monitor" },
];

function progress(health: string, runState = "idle") {
  return { health, run_state: runState, in_flight_work: 0, last_progress_at_ms: Date.now() - 60_000, last_progress_event: "unchanged" };
}

const AGENTS: ConsoleAgent[] = TEAM.map((member) => ({
  identity: member.identity,
  agent_id: member.identity,
  member_id: member.identity,
  label: member.label,
  kind: "mob_agent",
  role: member.role,
  group: member.group,
  state: "active",
  labels: { display_name: member.label, group: member.group },
  response_phase: member.identity === "triage" ? "generating" : null,
  progress: member.identity === "delivery"
    ? progress("degraded")
    : member.identity === "gate"
      ? progress("wedged", "run_open")
      : progress("healthy"),
}));

const NODES: ConsoleTopologyNode[] = [
  ...TEAM.map((member) => ({
    identity: key(member.identity),
    ref: { id: key(member.identity), authority: AUTHORITY, identity: member.identity },
    label: member.label,
    role: member.role,
    group: member.group,
    labels: { display_name: member.label, group: member.group },
    wired_to: [],
  })),
  {
    identity: key("ticket-4411"),
    ref: { id: key("ticket-4411"), authority: AUTHORITY, identity: "ticket-4411" },
    label: "ticket 4411",
    role: "specialist",
    group: "Domains",
    labels: { display_name: "ticket 4411", spawned_by: "support" },
    wired_to: [],
  },
  {
    identity: key("ledger", "finance"),
    ref: { id: key("ledger", "finance"), authority: "finance", identity: "ledger" },
    label: "Ledger",
    role: "specialist",
    labels: { topology_stub: "true" },
    presentation: { crossScope: true, scopeId: "finance", scopeLabel: "finance" },
    wired_to: [],
  },
];

const wired = (a: string, b: string): ConsoleTopologyLink => ({
  from: key(a),
  to: key(b),
  actual: true,
  declared: true,
  desired: true,
});

const LINKS: ConsoleTopologyLink[] = [
  wired("router", "triage"),
  wired("router", "billing"),
  wired("router", "delivery"),
  wired("router", "support"),
  wired("support", "triage"),
  wired("billing", "gate"),
  wired("delivery", "gate"),
  // Declared and wanted, not wired: a degraded wiring.
  { from: key("billing"), to: key("delivery"), actual: false, declared: true, desired: true },
  // An operator link to another team.
  { from: key("billing"), to: key("ledger", "finance"), actual: false, desired: true, operatorAdded: true, cross: true },
];

const ROUTES: ConsoleTopologyRoute[] = [
  { key: "billing-events", channel: "billing events", recipient: "billing", sink: "mob_member", deliveries: { sent: 40, failed: 0 } },
  { key: "support-inbox", channel: "support inbox", recipient: "triage", sink: "mob_member", deliveries: { sent: 22, failed: 0 } },
  { key: "nightly-digest", channel: "nightly digest", recipient: "health-monitor", sink: "mob_member", deliveries: { sent: 6, failed: 2 } },
];

const MANAGEMENT: TopologyManagementState = {
  revision: 7,
  policy: {
    mode: "editable",
    capabilities: {
      connect: { state: "allowed" },
      disconnect: { state: "allowed" },
      reconnect: { state: "allowed" },
    },
  },
  affordances: [
    { edge: { from: key("router"), to: key("triage") }, state: "connected", actions: { disconnect: { state: "allowed" } } },
    { edge: { from: key("health-monitor"), to: key("router") }, state: "disconnected", actions: { connect: { state: "allowed" } } },
  ],
  operations: [
    {
      operationId: "gallery-connect",
      action: "connect",
      status: "running",
      edge: { from: key("health-monitor"), to: key("router") },
    },
  ],
};

let frameSeq = 0;
function message(from: string, to: string, atMs: number): ConsoleFrame {
  frameSeq += 1;
  return {
    id: `gallery-message-${frameSeq}`,
    event: "system_notice",
    identity: to,
    timestampMs: atMs,
    data: {
      kind: "comms",
      blocks: [{
        type: "comms",
        kind: "message",
        direction: "incoming",
        peer: { display_name: `${AUTHORITY}/${from}` },
        delivery_id: `gallery-delivery-${frameSeq}`,
      }],
    },
  };
}

function initialFrames(now: number): ConsoleFrame[] {
  const frames: ConsoleFrame[] = [];
  for (let index = 0; index < 12; index += 1) frames.push(message("triage", "support", now - (index + 1) * 4 * 60_000));
  for (let index = 0; index < 3; index += 1) frames.push(message("router", "billing", now - (index + 2) * 9 * 60_000));
  frames.push({ id: "gallery-run", event: "run_started", identity: "triage", timestampMs: now - 40_000, data: {} });
  frames.push({
    id: "gallery-tool",
    event: "tool_execution_started",
    identity: "triage",
    timestampMs: now - 12_000,
    data: { name: "refunds.lookup", tool_call_id: "gallery-tool-call" },
  });
  return frames.sort((left, right) => (right.timestampMs || 0) - (left.timestampMs || 0));
}

/** The fixture team, with a message from Triage to Support every few seconds. */
function TopologyGalleryPanel(): React.JSX.Element {
  const [frames, setFrames] = React.useState<ConsoleFrame[]>(() => initialFrames(Date.now()));
  React.useEffect(() => {
    const timer = window.setInterval(() => {
      setFrames((current) => [message("triage", "support", Date.now()), ...current].slice(0, 200));
    }, 5_000);
    return () => window.clearInterval(timer);
  }, []);
  return (
    <div style={{ height: 860, display: "flex", border: "1px solid transparent" }}>
      <TopologyPanel
        nodes={NODES}
        agents={AGENTS}
        activity={frames}
        links={LINKS}
        routes={ROUTES}
        routesAccess="loaded"
        memoryScopes={[
          { identity: "triage", records: 12 },
          { identity: "billing", records: 8 },
          { identity: "support", records: 5 },
        ]}
        stores={[{ id: "team", label: "Team memory", records: 120 }]}
        management={MANAGEMENT}
        onRequestMutation={() => undefined}
        onSelectAgent={() => undefined}
        onOpenAgentDetails={() => undefined}
        onOpenMemory={() => undefined}
        updatedAtMs={Date.now() - 12_000}
        teamName="Operations team"
        teamKey="operations"
        canWireMembers
      />
    </div>
  );
}

export const gallerySections: Array<{
  id: string;
  title: string;
  order?: number;
  description?: string;
  render: () => React.ReactNode;
}> = [
  {
    id: "topology",
    title: "Topology",
    order: 100,
    description: "The Topology panel on a fixture team: lanes, every relation kind, live and health states, the drawer.",
    render: () => <TopologyGalleryPanel />,
  },
];
