/**
 * Console schedules in the stock console: an agent's details pane lists the
 * agent's schedules where the gateway advertises `meta/schedule/list`, lets a
 * viewer who may message the agent change them where it advertises the
 * writes, and shows the list read-only to anyone else. Each command names
 * the agent as `identity`.
 */
import React from "react";
import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { MetaConsoleTransport } from "./headless";

const AGENT = "alpha";
const SCOPE = "schedules-principal";

type Row = Record<string, unknown>;

interface Harness extends MetaConsoleTransport {
  methods: string[];
  canSend: boolean;
  accessEnabled: boolean;
  schedules: Row[];
  commands: Array<{ command: string; params: Record<string, unknown> }>;
}

function transport(): Harness {
  const harness: Harness = {
    methods: Object.values(CONSOLE_RPC_METHODS),
    canSend: true,
    accessEnabled: false,
    schedules: [],
    commands: [],
    loadExperience: async () => ({
      contract_version: "test",
      runtime_id: "schedules-runtime",
      storage_scope: SCOPE,
      console_config: {},
      console_policy: {},
      ...(harness.accessEnabled ? { access: { enabled: true, subject: "viewer" } } : {}),
      runtime_capabilities: {
        can_spawn_members: false,
        can_send_messages: true,
        can_retire_members: true,
      },
      agent_sidebar: {
        live_snapshot: {
          agents: [{
            identity: AGENT,
            member_id: AGENT,
            agent_id: AGENT,
            label: "Alpha",
            kind: "mob_agent",
            role: "worker",
            state: "active",
            addressable: true,
            affordances: { can_send_message: harness.canSend, can_retire: true },
            model_capabilities: { image_input: false },
          }],
        },
      },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: harness.methods }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => undefined,
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => {
      const params = (input.params || {}) as Record<string, unknown>;
      harness.commands.push({ command: input.command, params });
      if (input.command === "listSchedules") {
        return { command: input.command, accepted: true, result: { identity: AGENT, schedules: harness.schedules } } as never;
      }
      if (input.command === "createSchedule") {
        const created = {
          schedule_id: "sch_1",
          identity: AGENT,
          name: null,
          content: params.content,
          trigger: params.trigger,
          phase: "active",
          revision: 1,
          sender: "viewer",
          next_due_at_utc: (params.trigger as { due_at_utc?: string }).due_at_utc ?? null,
        };
        harness.schedules = [created];
        return { command: input.command, accepted: true, result: created } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
  return harness;
}

function seedDock(): void {
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", SCOPE])}`, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{
      id: "panel-1",
      mode: "console",
      target: { id: `inspect:${AGENT}`, kind: "identity-inspect", title: "Alpha Details", identity: AGENT, memberId: AGENT },
    }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
}

async function settle(ms: number): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

function present(view: ReturnType<typeof render>, testId: string): boolean {
  return view.queryByTestId(testId) !== null;
}

describe("console schedules", () => {
  beforeEach(() => {
    window.localStorage.clear();
    seedDock();
  });
  afterEach(() => window.localStorage.clear());

  it("lets a viewer who may message the agent schedule a message from its details", async () => {
    const harness = transport();
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await view.findByTestId(`inspect-panel:${AGENT}`);
    const create = await view.findByTestId(`schedules-new:${AGENT}`, undefined, { timeout: 5_000 });
    await waitFor(() => expect(present(view, `schedules-empty:${AGENT}`)).toBe(true));
    expect(present(view, `schedules-note:${AGENT}`)).toBe(false);

    fireEvent.click(create);
    const form = view.getByTestId(`schedule-form:${AGENT}`);
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Post the summary" } });
    fireEvent.submit(form);

    await view.findByTestId("schedule:sch_1", undefined, { timeout: 5_000 });
    const created = harness.commands.find((entry) => entry.command === "createSchedule");
    expect(created?.params.identity).toBe(AGENT);
    expect(created?.params.content).toBe("Post the summary");
    expect((created?.params.trigger as { type?: string } | undefined)?.type).toBe("once");
    expect(
      harness.commands
        .filter((entry) => entry.command === "listSchedules")
        .every((entry) => entry.params.identity === AGENT),
    ).toBe(true);
    view.unmount();
  }, 30_000);

  it("shows no schedules where the gateway does not offer them", async () => {
    const harness = transport();
    harness.methods = Object.values(CONSOLE_RPC_METHODS).filter((method) => !method.startsWith("meta/schedule/"));
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await view.findByTestId(`inspect-panel:${AGENT}`);
    await settle(300);
    expect(present(view, `schedules:${AGENT}`)).toBe(false);
    expect(harness.commands.some((entry) => entry.command === "listSchedules")).toBe(false);
    view.unmount();
  }, 30_000);

  it("shows the list read-only to a viewer who may not message the agent", async () => {
    const harness = transport();
    harness.accessEnabled = true;
    harness.canSend = false;
    harness.schedules = [{
      schedule_id: "sch_9",
      identity: AGENT,
      name: "Digest",
      content: "Send the digest",
      trigger: { type: "once", due_at_utc: "2030-01-01T09:00:00Z" },
      phase: "active",
      revision: 1,
      sender: null,
      sender_hidden: true,
      next_due_at_utc: "2030-01-01T09:00:00Z",
    }];
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await view.findByTestId("schedule:sch_9", undefined, { timeout: 5_000 });
    expect(view.getByTestId(`schedules-note:${AGENT}`).textContent).toContain("not message it");
    expect(present(view, `schedules-new:${AGENT}`)).toBe(false);
    expect(present(view, "schedule-edit:sch_9")).toBe(false);
    expect(present(view, "schedule-pause:sch_9")).toBe(false);
    expect(present(view, "schedule-delete:sch_9")).toBe(false);
    expect(present(view, "schedule-sender:sch_9")).toBe(false);
    view.unmount();
  }, 30_000);

  it("shows the list read-only where the gateway offers no writes", async () => {
    const harness = transport();
    harness.methods = [
      ...Object.values(CONSOLE_RPC_METHODS).filter((method) => !method.startsWith("meta/schedule/")),
      CONSOLE_RPC_METHODS.scheduleList,
    ];
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await view.findByTestId(`schedules-note:${AGENT}`, undefined, { timeout: 5_000 });
    expect(view.getByTestId(`schedules-note:${AGENT}`).textContent).toBe(
      "Schedules can be viewed but not changed on this console.",
    );
    expect(present(view, `schedules-new:${AGENT}`)).toBe(false);
    view.unmount();
  }, 30_000);
});
