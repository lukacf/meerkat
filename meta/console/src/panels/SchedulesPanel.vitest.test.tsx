/**
 * The Schedules workbench panel (console v2 spec B10): every visible agent's
 * schedules, read four at a time and kept for 15 seconds, planned in the
 * browser, as an agenda, a week grid or a list with the viewer's actions.
 */
import React from "react";
import { act, fireEvent, render, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { SchedulesPanel, type SchedulesPanelProps } from "./SchedulesPanel";
import { DEMO_SCHEDULE_AGENTS, createDemoScheduleTransport } from "./schedule-fixtures";
import { agentDirectoryEntry } from "../lib/agents";
import type { ConsoleAgent } from "../types";
import type { ConsoleScheduleCommand } from "../lib/schedules";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

// Monday 4 May 2026, 08:00 on the viewer's clock.
const NOW = new Date(2026, 4, 4, 8, 0, 0);
const OFFER = { list: true, write: true };

function present(view: ReturnType<typeof render>, testId: string): boolean {
  return view.queryByTestId(testId) !== null;
}

function panel(props: Partial<SchedulesPanelProps> & Pick<SchedulesPanelProps, "run">, agents: ConsoleAgent[] = DEMO_SCHEDULE_AGENTS) {
  const directory = createAgentDirectory(agents.map(agentDirectoryEntry));
  return (
    <AgentDirectoryProvider directory={directory}>
      <SchedulesPanel
        agents={agents}
        offer={OFFER}
        readOnly={false}
        accessEnforcing={false}
        now={() => new Date(NOW.getTime())}
        timezone="Europe/Paris"
        {...props}
      />
    </AgentDirectoryProvider>
  );
}

function demo(options: Parameters<typeof createDemoScheduleTransport>[1] = {}) {
  const transport = createDemoScheduleTransport(NOW, options);
  const calls: Array<{ identity: string; command: ConsoleScheduleCommand; params: Record<string, unknown> }> = [];
  const run = vi.fn(async (identity: string, command: ConsoleScheduleCommand, params: Record<string, unknown>) => {
    calls.push({ identity, command, params });
    return transport.run(identity, command, params);
  });
  return { transport, calls, run };
}

async function loaded(view: ReturnType<typeof render>) {
  await waitFor(() => expect(present(view, "schedules-agenda")).toBe(true));
}

describe("SchedulesPanel", () => {
  beforeEach(() => window.localStorage.clear());

  it("reads each agent's list four at a time and keeps it for 15 seconds", async () => {
    const agents = Array.from({ length: 6 }, (_, index) => ({
      ...DEMO_SCHEDULE_AGENTS[0],
      identity: `agent-${index}`,
      member_id: `agent-${index}`,
      agent_id: `agent-${index}`,
      label: `Agent ${index}`,
    }));
    const pending: Array<() => void> = [];
    let open = 0;
    let most = 0;
    const run = vi.fn((identity: string) => {
      open += 1;
      most = Math.max(most, open);
      return new Promise((resolve) => {
        pending.push(() => {
          open -= 1;
          resolve({ identity, schedules: [] });
        });
      });
    });
    let clock = NOW.getTime();
    const view = render(panel({ run, now: () => new Date(clock), refreshToken: 1 }, agents));
    await waitFor(() => expect(run.mock.calls.length).toBe(4));
    expect(present(view, "schedules-loading")).toBe(true);
    while (pending.length) {
      await act(async () => {
        pending.shift()?.();
      });
    }
    await waitFor(() => expect(run.mock.calls.length).toBe(6));
    while (pending.length) {
      await act(async () => {
        pending.shift()?.();
      });
    }
    expect(most).toBe(4);
    await waitFor(() => expect(present(view, "schedules-empty")).toBe(true));
    // A refresh inside 15 seconds reads nothing; after it, every list again.
    view.rerender(panel({ run, now: () => new Date(clock), refreshToken: 2 }, agents));
    expect(run.mock.calls.length).toBe(6);
    clock += 16_000;
    view.rerender(panel({ run, now: () => new Date(clock), refreshToken: 3 }, agents));
    await waitFor(() => expect(run.mock.calls.length).toBe(10));
    expect(run.mock.calls.every((call) => call[1] === "listSchedules")).toBe(true);
  });

  it("leads with the next run and counts active and paused schedules", async () => {
    const { run } = demo();
    const view = render(panel({ run }));
    await loaded(view);
    const summary = view.getByTestId("schedules-summary").textContent || "";
    expect(summary.startsWith("9 schedules · 7 active · next: Heartbeat check in")).toBe(true);
    const activeTile = view.getByTestId("schedules-glance:active");
    expect(activeTile.textContent?.includes("7")).toBe(true);
    expect(activeTile.textContent?.includes("of 9")).toBe(true);
    expect(view.getByTestId("schedules-glance:paused").textContent?.includes("2")).toBe(true);
    expect(view.getByTestId("schedules-glance:next").textContent?.includes("Heartbeat check · Health Monitor")).toBe(true);
    // The paused filter scopes the views.
    fireEvent.click(activeTile);
    expect(activeTile.getAttribute("aria-pressed")).toBe("true");
  });

  it("draws the agenda by agent: shapes by kind, the next run in the accent, a dense repeat as a band", async () => {
    const { run } = demo();
    const view = render(panel({ run }));
    await loaded(view);
    expect(view.getByTestId("schedules-view:agenda").getAttribute("aria-pressed")).toBe("true");
    const morning = view.getByTestId("schedules-agenda-run:sch_morning_brief:0");
    expect(morning.getAttribute("data-shape")).toBe("diamond");
    expect(morning.classList.contains("is-live")).toBe(true);
    expect(morning.getAttribute("aria-label")?.startsWith("Morning brief · ")).toBe(true);
    expect(morning.getAttribute("aria-label")?.endsWith("Europe/Paris")).toBe(true);
    expect(view.getByTestId("schedules-agenda-run:sch_inbox_sweep:0").getAttribute("data-shape")).toBe("tick");
    // Tomorrow's once run is beyond the next 24 hours.
    expect(present(view, "schedules-agenda-run:sch_invoice_run:0")).toBe(false);
    expect(view.getByTestId("schedules-agenda-run:sch_heartbeat:band").getAttribute("aria-label")?.startsWith("Heartbeat check · every 10 min")).toBe(true);
    expect(present(view, "schedules-agenda-row:health-monitor")).toBe(true);
    // Paused: hollow, never the accent.
    fireEvent.click(view.getByTestId("schedules-range:week"));
    expect(view.getByTestId("schedules-agenda-run:sch_invoice_run:0").getAttribute("data-shape")).toBe("square");
    const handover = view.getByTestId("schedules-agenda-run:sch_weekend_handover:0");
    expect(handover.getAttribute("data-paused")).toBe("true");
    expect(handover.classList.contains("is-live")).toBe(false);
    // A run opens its schedule in the drawer.
    fireEvent.click(view.getByTestId("schedules-agenda-run:sch_invoice_run:0"), { clientX: 0 });
    fireEvent.keyDown(view.getByTestId("schedules-agenda-run:sch_invoice_run:0"), { key: "Enter" });
    await waitFor(() => expect(present(view, "schedules-detail:sch_invoice_run")).toBe(true));
    const detail = view.getByTestId("schedules-detail:sch_invoice_run");
    expect(detail.textContent?.includes("Run the invoice batch")).toBe(true);
    expect(within(detail).getByRole("heading", { level: 3 }).textContent).toBe("Invoice run");
  });

  it("shows a week of planned runs per hour", async () => {
    const { run } = demo();
    const view = render(panel({ run }));
    await loaded(view);
    fireEvent.click(view.getByTestId("schedules-view:week"));
    const week = view.getByTestId("schedules-week");
    expect(week.querySelectorAll("tbody tr").length).toBe(7);
    expect((week.querySelector("svg")?.getAttribute("aria-label") || "").includes("planned runs over the next 7 days, busiest")).toBe(true);
  });

  it("lists schedules by agent with the viewer's actions, and confirms a delete", async () => {
    const { run, calls } = demo();
    const view = render(panel({ run }));
    await loaded(view);
    fireEvent.click(view.getByTestId("schedules-view:list"));
    const support = view.getByTestId("schedules-group:support");
    expect(within(support).getAllByTestId(/^schedules-row:/).length).toBe(3);
    const sweep = view.getByTestId("schedules-row:sch_inbox_sweep");
    expect(sweep.textContent?.includes("Every 2 hours from")).toBe(true);
    expect(sweep.textContent?.includes("Sends as Coordinator")).toBe(true);
    expect(view.getByTestId("schedules-phase:sch_weekend_handover").textContent).toBe("Paused");
    // Raw keys stay in tooltips.
    expect(sweep.textContent?.includes("sch_inbox_sweep")).toBe(false);

    fireEvent.click(view.getByTestId("schedules-pause:sch_inbox_sweep"));
    await waitFor(() => expect(view.getByTestId("schedules-phase:sch_inbox_sweep").textContent).toBe("Paused"));
    expect(calls.find((call) => call.command === "pauseSchedule")).toEqual({
      identity: "support",
      command: "pauseSchedule",
      params: { schedule_id: "sch_inbox_sweep" },
    });

    await waitFor(() => expect((view.getByTestId("schedules-delete:sch_invoice_run") as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(view.getByTestId("schedules-delete:sch_invoice_run"));
    expect(calls.some((call) => call.command === "deleteSchedule")).toBe(false);
    fireEvent.click(view.getByTestId("schedules-delete-confirm:sch_invoice_run"));
    await waitFor(() => expect(present(view, "schedules-row:sch_invoice_run")).toBe(false));
    expect(calls.filter((call) => call.command === "deleteSchedule").map((call) => call.identity)).toEqual(["billing"]);
  });

  it("creates a schedule from the drawer form for the chosen agent", async () => {
    const { run, calls } = demo();
    const view = render(panel({ run }));
    await loaded(view);
    fireEvent.click(view.getByTestId("schedules-view:list"));
    fireEvent.click(view.getByTestId("schedules-new"));
    const drawer = view.getByTestId("schedules-drawer");
    expect(within(drawer).getByRole("heading", { level: 3 }).textContent).toBe("New schedule");
    fireEvent.change(within(drawer).getByTestId("schedule-form-agent"), { target: { value: "billing" } });
    const form = view.getByTestId("schedule-form:billing");
    fireEvent.change(within(form).getByTestId("schedule-form-name"), { target: { value: "Ledger check" } });
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Check the ledger" } });
    fireEvent.submit(form);
    await waitFor(() => expect(present(view, "schedules-drawer")).toBe(false));
    const created = calls.find((call) => call.command === "createSchedule");
    expect(created?.identity).toBe("billing");
    expect(created?.params.content).toBe("Check the ledger");
    expect((created?.params.trigger as { type?: string }).type).toBe("once");
    await waitFor(() => expect(view.getByTestId("schedules-group:billing").textContent?.includes("Ledger check")).toBe(true));
  });

  it("keeps the form with the refusal when the gateway refuses a write", async () => {
    const { run } = demo();
    const refusing = vi.fn(async (identity: string, command: ConsoleScheduleCommand, params: Record<string, unknown>) => {
      if (command === "createSchedule") {
        throw Object.assign(new Error("refused"), { rpcError: { code: -32004, message: "refused", data: { kind: "governed_target" } } });
      }
      return run(identity, command, params);
    });
    const view = render(panel({ run: refusing }));
    await loaded(view);
    fireEvent.click(view.getByTestId("schedules-new-tool"));
    const form = view.getByTestId(/^schedule-form:/);
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Check the queue" } });
    fireEvent.submit(form);
    await waitFor(() => expect(present(view, "schedule-form-error")).toBe(true));
    expect(view.getByTestId("schedule-form-error").textContent).toBe(
      "This agent admits only governed input, which needs you present, so a schedule cannot send to it.",
    );
    expect(present(view, "schedules-drawer")).toBe(true);
  });

  it("counts an agent whose list it may not read, and says when a read failed", async () => {
    const { run } = demo({ refuse: ["billing"] });
    const failing = vi.fn(async (identity: string, command: ConsoleScheduleCommand, params: Record<string, unknown>) => {
      if (identity === "health-monitor") throw new Error("network down");
      return run(identity, command, params);
    });
    const view = render(panel({ run: failing }));
    await loaded(view);
    expect(view.getByTestId("schedules-summary").textContent?.includes("1 agent not readable")).toBe(true);
    await waitFor(() => expect(present(view, "schedules-panel-error")).toBe(true));
    expect(view.getByTestId("schedules-panel-error").textContent).toBe("Couldn't refresh the schedules of 1 agent.");
    expect(view.getByTestId("schedules-panel-error").getAttribute("role")).toBe("alert");
  });

  it("offers no changes on a read-only console", async () => {
    const { run } = demo();
    const view = render(panel({ run, readOnly: true }));
    await loaded(view);
    fireEvent.click(view.getByTestId("schedules-view:list"));
    expect(present(view, "schedules-new")).toBe(false);
    expect(present(view, "schedules-new-tool")).toBe(false);
    expect(present(view, "schedules-pause:sch_inbox_sweep")).toBe(false);
    expect(view.getByTestId("schedules-group:support").textContent?.includes("read-only")).toBe(true);
  });

  it("explains a console without a schedule host and reads nothing", () => {
    const { run } = demo();
    const view = render(panel({ run, offer: { list: false, write: false } }));
    expect(view.getByTestId("schedules-not-set-up").textContent?.includes("Schedules aren't set up on this console.")).toBe(true);
    expect(view.getByRole("heading", { level: 2 }).textContent).toBe("Schedules");
    expect(run.mock.calls.length).toBe(0);
  });
});
