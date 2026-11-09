// Dev-only kit gallery sections for Schedules (console v2 spec B10, shots
// S1-S3): the real Schedules panel in a pane-sized frame, fed by the demo
// schedule transport (the seeded preview has no schedule host). Found by
// console/src/dev/app-gallery.ts; nothing here reaches production.

import React from "react";
import type { GallerySection } from "@console-components";

import { agentDirectoryEntry } from "../lib/agents";
import { SchedulesPanel, type SchedulesView } from "./SchedulesPanel";
import { DEMO_SCHEDULE_AGENTS, createDemoScheduleTransport } from "./schedule-fixtures";
import {
  AgentDirectoryProvider,
  PanelPreferencesProvider,
  createAgentDirectory,
} from "../../../packages/console-components/src/panel-kit";
import { Specimen } from "../../../packages/console-components/src/panel-kit/gallery/specimen";

const DIRECTORY = createAgentDirectory(DEMO_SCHEDULE_AGENTS.map(agentDirectoryEntry));

/** Preferences kept in memory, so each section opens on its own view. */
function memoryStorage() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
    removeItem: (key: string) => void values.delete(key),
  };
}

function DemoSchedules({ id, view, openNew }: { id: string; view: SchedulesView; openNew?: string }) {
  const transport = React.useMemo(() => createDemoScheduleTransport(new Date()), []);
  const storage = React.useMemo(memoryStorage, []);
  return (
    <PanelPreferencesProvider scope={`gallery-${id}`} storage={storage}>
      <AgentDirectoryProvider directory={DIRECTORY}>
        <div className="schedules-gallery__pane" style={{ display: "flex", width: 1020, maxWidth: "100%", height: 760 }}>
          <SchedulesPanel
            agents={DEMO_SCHEDULE_AGENTS}
            offer={{ list: true, write: true }}
            readOnly={false}
            accessEnforcing={false}
            run={transport.run}
            defaultView={view}
            openNew={openNew}
          />
        </div>
      </AgentDirectoryProvider>
    </PanelPreferencesProvider>
  );
}

export const gallerySections: GallerySection[] = [
  {
    id: "schedules-agenda",
    title: "Schedules: Agenda",
    order: 200,
    description:
      "Shot S1. Nine demo schedules across four agents on the next 24 hours: once squares, repeat ticks, calendar diamonds, each next run in the accent, a 10-minute heartbeat as a band. The 7 days range adds the paused schedules' hollow runs.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoSchedules id="schedules-agenda" view="agenda" />
      </Specimen>
    ),
  },
  {
    id: "schedules-week",
    title: "Schedules: Week",
    order: 210,
    description: "Shot S2. Planned runs per hour over the next seven days, in the ink ramp; paused schedules are not counted.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoSchedules id="schedules-week" view="week" />
      </Specimen>
    ),
  },
  {
    id: "schedules-list",
    title: "Schedules: List with the form",
    order: 220,
    description: "Shot S3. Schedules grouped by agent with their actions, and the New schedule form open in the drawer.",
    render: () => (
      <Specimen bare width={1020}>
        <DemoSchedules id="schedules-list" view="list" openNew="billing" />
      </Specimen>
    ),
  },
];
