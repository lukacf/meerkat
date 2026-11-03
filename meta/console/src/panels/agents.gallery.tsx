// Dev-only kit gallery sections for the agents panels (console v2 spec B8,
// B9, B11; shots R1, H1 and I2, with R2, R3 and I1 as fixture twins of the
// seeded preview's shots): the real Roster, Health and Agent details in
// pane-sized frames over a demo team and a fixed activity ledger. The seeded
// preview cannot produce degraded or stalled health, nor schedules. Found by
// console/src/dev/app-gallery.ts; nothing here reaches production.

import React from "react";
import type { GallerySection } from "@console-components";
import type { IdentityInspectViewState } from "@console-core";

import { agentDirectoryEntry } from "../lib/agents";
import { ActivityLedgerProvider } from "../lib/activity-ledger";
import { consoleScheduleEditing } from "../lib/schedules";
import { AgentDetailsPanel } from "./AgentDetailsPanel";
import { HealthPanel } from "./HealthPanel";
import { RosterPanel } from "./RosterPanel";
import { SchedulesSection } from "./SchedulesSection";
import { createDemoLedger, demoTeamAgents, demoTeamHealthRows } from "./agents-fixtures";
import { DEMO_SCHEDULE_AGENTS, createDemoScheduleTransport } from "./schedule-fixtures";
import {
  AgentDirectoryProvider,
  ConsoleNavigationProvider,
  PanelPreferencesProvider,
  createAgentDirectory,
  type ConsoleNavigation,
} from "../../../packages/console-components/src/panel-kit";
import { Specimen } from "../../../packages/console-components/src/panel-kit/gallery/specimen";
import { identityStateLabel } from "@console-core";

/** Preferences kept in memory, so each section opens on its default view. */
function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
    removeItem: (key: string) => void values.delete(key),
  };
}

/** Names are links in the console; here they open nothing. */
const NAVIGATION: ConsoleNavigation = { openAgent: () => {}, openPanel: () => {} };

/** The demo team's world: names, the ledger, links, in-memory preferences. */
function DemoTeam({ id, now, aliases, children }: { id: string; now: number; aliases?: Record<string, string>; children: React.ReactNode }) {
  const storage = React.useMemo(memoryStorage, []);
  const ledger = React.useMemo(() => createDemoLedger(now, aliases), [now, aliases]);
  const directory = React.useMemo(
    () => createAgentDirectory([...demoTeamAgents(now), ...DEMO_SCHEDULE_AGENTS].map(agentDirectoryEntry)),
    [now],
  );
  return (
    <PanelPreferencesProvider scope={`gallery-${id}`} storage={storage}>
      <AgentDirectoryProvider directory={directory}>
        <ActivityLedgerProvider ledger={ledger}>
          <ConsoleNavigationProvider value={NAVIGATION}>{children}</ConsoleNavigationProvider>
        </ActivityLedgerProvider>
      </AgentDirectoryProvider>
    </PanelPreferencesProvider>
  );
}

function Pane({ width = 1020, height = 760, children }: { width?: number; height?: number; children: React.ReactNode }) {
  return <div style={{ display: "flex", width, maxWidth: "100%", height }}>{children}</div>;
}

function useFixedNow(): number {
  return React.useMemo(() => Date.now(), []);
}

function DemoRoster({ id, width, openId }: { id: string; width?: number; openId?: string }) {
  const now = useFixedNow();
  const agents = React.useMemo(() => demoTeamAgents(now), [now]);
  return (
    <DemoTeam id={id} now={now}>
      <Pane width={width}>
        <RosterPanel
          agents={agents}
          now={() => now}
          defaultOpenMemberId={openId}
          canResetLifecycle
          onSelect={() => {}}
          onChat={() => {}}
          onDetails={() => {}}
          onLifecycle={() => {}}
        />
      </Pane>
    </DemoTeam>
  );
}

function DemoHealth() {
  const now = useFixedNow();
  const agents = React.useMemo(() => demoTeamAgents(now), [now]);
  const rows = React.useMemo(() => demoTeamHealthRows(now), [now]);
  return (
    <DemoTeam id="agents-health" now={now}>
      <Pane height={1000}>
        <HealthPanel
          identities={rows}
          agents={agents}
          stateLabel={identityStateLabel}
          team={{ running: true, loadedModules: ["router", "delivery"], loadedModuleCount: 2 }}
          now={() => now}
          onOpen={() => {}}
        />
      </Pane>
    </DemoTeam>
  );
}

function demoInspect(identity: string, name: string, role: string, peers: string[], preview: string): IdentityInspectViewState {
  return {
    identity,
    display_name: name,
    role,
    state: "active",
    addressability: "addressable",
    labels: { display_name: name },
    continuity: { generation: 3, session_id: `session-${identity}-01JC7Q2M8V`, agent_runtime_id: identity },
    topology_peers: peers,
    peer_reachable_count: peers.length,
    output_preview: preview,
    is_final: true,
  } as IdentityInspectViewState;
}

const SCHEDULE_ALIASES = { coordinator: "dispatch", support: "intake" };

function DemoDetails({ withSchedules }: { withSchedules?: boolean }) {
  const now = useFixedNow();
  const team = React.useMemo(() => demoTeamAgents(now), [now]);
  const transport = React.useMemo(() => createDemoScheduleTransport(new Date(now)), [now]);
  if (withSchedules) {
    const coordinator = { ...DEMO_SCHEDULE_AGENTS[0], wired_to: ["support", "billing", "health-monitor"] };
    const agents = [coordinator, ...DEMO_SCHEDULE_AGENTS.slice(1)];
    return (
      <DemoTeam id="agents-details-schedules" now={now} aliases={SCHEDULE_ALIASES}>
        <Pane height={1300}>
          <AgentDetailsPanel
            identity="coordinator"
            inspect={demoInspect(
              "coordinator",
              "Coordinator",
              "coordinator",
              ["support", "billing", "health-monitor"],
              "Monday summary sent: 14 tickets closed, 3 waiting on Billing, the release notes are with Support.",
            )}
            agent={coordinator}
            agents={agents}
            stateLabel="active"
            now={() => now}
            onOpenChat={() => {}}
            actions={{
              respawn: { label: "Respawn", onClick: () => {} },
              reset: { label: "Reset", onClick: () => {} },
              retire: { label: "Retire", onClick: () => {} },
            }}
          >
            <SchedulesSection
              identity="coordinator"
              editing={consoleScheduleEditing({ offer: { list: true, write: true }, readOnly: false, accessEnforcing: false, agent: coordinator })}
              run={(command, params) => transport.run("coordinator", command, params)}
              now={() => new Date(now)}
            />
          </AgentDetailsPanel>
        </Pane>
      </DemoTeam>
    );
  }
  const research = team.find((agent) => agent.identity === "research");
  return (
    <DemoTeam id="agents-details" now={now}>
      <Pane height={1100}>
        <AgentDetailsPanel
          identity="research"
          inspect={demoInspect(
            "research",
            "Research Analyst",
            "analyst",
            ["dispatch", "knowledge"],
            "Pulled the carrier delay reports for the last two weeks; drafting the summary for Dispatch.",
          )}
          agent={research}
          agents={team}
          work={[
            { id: "wi-41", title: "Collect the carrier delay reports", status: "in_progress", claimedAt: new Date(now - 2 * 3_600_000).toISOString() },
            { id: "wi-44", title: "Summarize the delay trends for Dispatch", status: "ready" },
            { id: "wi-38", title: "Check the holiday stock for the top 20 products", status: "blocked" },
          ]}
          stateLabel="active"
          now={() => now}
          onOpenChat={() => {}}
          actions={{
            respawn: { label: "Respawn", onClick: () => {} },
            reset: { label: "Reset", onClick: () => {} },
            retire: { label: "Retire", onClick: () => {} },
          }}
        />
      </Pane>
    </DemoTeam>
  );
}

export const gallerySections: GallerySection[] = [
  {
    id: "agents-roster",
    title: "Roster: cards",
    order: 100,
    description:
      "Shot R1. The demo team as cards by section: two agents working now (the accent rule and the live pill), one degraded, one stalled, a side chat and a retired member; the glance strip filters.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoRoster id="agents-roster" />
      </Specimen>
    ),
  },
  {
    id: "agents-roster-drawer",
    title: "Roster: the detail drawer",
    order: 101,
    description: "Shot R2 on the fixture: a working agent's drawer with its actions, NOW, a day of activity, the hour-by-hour strip and its connections.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoRoster id="agents-roster-drawer" openId="research" />
      </Specimen>
    ),
  },
  {
    id: "agents-roster-narrow",
    title: "Roster: a 330 px pane",
    order: 102,
    description: "Shot R3 on the fixture: below 720 px the Roster lists rows; under 480 px the glance strip is a 2 x 2 grid and the search folds.",
    render: () => (
      <Specimen bare width={330}>
        <DemoRoster id="agents-roster-narrow" width={330} />
      </Specimen>
    ),
  },
  {
    id: "agents-health",
    title: "Health",
    order: 110,
    description: "Shot H1. One tile per agent, the worst first: a stalled and a degraded agent, two working (health checked once idle), the rest healthy; the team's activity by hour, its state and modules.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoHealth />
      </Specimen>
    ),
  },
  {
    id: "agents-details",
    title: "Agent details: a working agent",
    order: 120,
    description: "Shot I1 on the fixture: a working agent's page, its day at a glance, what it is doing now, its activity, its ego graph and recent turns.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoDetails />
      </Specimen>
    ),
  },
  {
    id: "agents-details-schedules",
    title: "Agent details: with schedules",
    order: 121,
    description: "Shot I2. The coordinator's page with its schedules from the demo schedule transport: the next run, the next seven days and the list.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoDetails withSchedules />
      </Specimen>
    ),
  },
];
