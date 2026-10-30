/**
 * The schedules of one agent in its details pane: listed for every viewer,
 * changed only by one who may, with each refusal shown by its kind.
 */
import React from "react";
import { fireEvent, render, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { SchedulesSection } from "./SchedulesSection";
import type { ConsoleScheduleCommand } from "../lib/schedules";

type Row = Record<string, unknown>;
type Call = { command: string; params: Record<string, unknown> };

const NOW = new Date(2030, 0, 2, 8, 0, 0);
const now = () => new Date(NOW.getTime());
const verbatim = (iso: string) => iso;

function item(extra: Row = {}): Row {
  return {
    schedule_id: "sch_1",
    identity: "alpha",
    name: "Standup",
    content: "Post the standup summary",
    trigger: { type: "interval", start_at_utc: "2030-01-01T09:00:00Z", every_seconds: 7200 },
    phase: "active",
    revision: 1,
    sender: "console:shared",
    defined_at_utc: "2030-01-01T08:00:00Z",
    created_at_utc: "2030-01-01T08:00:00Z",
    updated_at_utc: "2030-01-01T08:00:00Z",
    next_due_at_utc: "2030-01-01T11:00:00Z",
    ...extra,
  };
}

function rpcError(code: number, data?: unknown): Error {
  return Object.assign(new Error(`rpc ${code}`), { rpcError: { code, message: `rpc ${code}`, data } });
}

/** A gateway that keeps the agent's schedules and records each call. */
function gateway(initial: Row[] = []) {
  const state = {
    items: [...initial],
    calls: [] as Call[],
    fail: null as null | ((call: Call) => Error | null),
  };
  const run = vi.fn(async (command: ConsoleScheduleCommand, params: Record<string, unknown>) => {
    const call = { command, params };
    state.calls.push(call);
    const failure = state.fail?.(call);
    if (failure) throw failure;
    const index = state.items.findIndex((entry) => entry.schedule_id === params.schedule_id);
    switch (command) {
      case "listSchedules":
        return { identity: "alpha", schedules: state.items };
      case "createSchedule": {
        const created = item({
          schedule_id: `sch_${state.items.length + 1}`,
          name: params.name ?? null,
          content: params.content,
          trigger: params.trigger,
        });
        state.items.push(created);
        return created;
      }
      case "updateSchedule":
        state.items[index] = {
          ...state.items[index],
          ...(params.content !== undefined ? { content: params.content } : {}),
          ...(params.name !== undefined ? { name: params.name } : {}),
          ...(params.trigger !== undefined ? { trigger: params.trigger } : {}),
        };
        return state.items[index];
      case "pauseSchedule":
        state.items[index] = { ...state.items[index], phase: "paused", next_due_at_utc: null };
        return state.items[index];
      case "resumeSchedule":
        state.items[index] = { ...state.items[index], phase: "active" };
        return state.items[index];
      case "deleteSchedule":
        state.items.splice(index, 1);
        return { identity: "alpha", schedule_id: params.schedule_id, deleted: true };
      default:
        throw new Error(`unexpected ${command}`);
    }
  });
  return { state, run };
}

function present(view: ReturnType<typeof render>, testId: string): boolean {
  return view.queryByTestId(testId) !== null;
}

function section(
  run: ReturnType<typeof gateway>["run"],
  canChange: boolean,
  extra: { note?: string | null; refreshToken?: unknown; now?: () => Date } = {},
) {
  return (
    <SchedulesSection
      identity="alpha"
      editing={{ canChange, note: extra.note ?? null }}
      run={run}
      refreshToken={extra.refreshToken}
      now={extra.now ?? now}
      timezone="UTC"
      formatTime={verbatim}
    />
  );
}

describe("SchedulesSection", () => {
  it("lists an agent's schedules read-only for a viewer who may not change them", async () => {
    const { run, state } = gateway([
      item(),
      item({ schedule_id: "sch_2", name: null, phase: "paused", sender: null, next_due_at_utc: null }),
      item({ schedule_id: "sch_3", sender: null, sender_hidden: true }),
    ]);
    const view = render(section(run, false, { note: "You may view this agent but not message it." }));
    await waitFor(() => expect(present(view, "schedule:sch_3")).toBe(true));

    expect(view.getByTestId("schedule-when:sch_1").textContent).toBe("Every 2 hours from 2030-01-01T09:00:00Z");
    expect(view.getByTestId("schedule-next:sch_1").textContent).toBe("Next: 2030-01-01T11:00:00Z");
    expect(view.getByTestId("schedule-sender:sch_1").textContent).toBe("Sends as console:shared");
    expect(view.getByTestId("schedule-phase:sch_2").textContent).toBe("Paused");
    expect(view.getByTestId("schedule:sch_2").textContent).toContain("Untitled schedule");
    expect(view.getByTestId("schedule-sender:sch_2").textContent).toBe("Sends as this console");
    expect(present(view, "schedule-sender:sch_3")).toBe(false);
    expect(present(view, "schedule-next:sch_2")).toBe(false);
    expect(view.getByTestId("schedules-note:alpha").textContent).toBe("You may view this agent but not message it.");
    for (const control of [
      "schedules-new:alpha",
      "schedule-edit:sch_1",
      "schedule-pause:sch_1",
      "schedule-resume:sch_2",
      "schedule-delete:sch_1",
    ]) {
      expect(present(view, control)).toBe(false);
    }
    expect(state.calls.map((call) => call.command)).toEqual(["listSchedules"]);
  });

  it("creates a schedule from the form and lists it", async () => {
    const { run, state } = gateway();
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedules-empty:alpha")).toBe(true));

    fireEvent.click(view.getByTestId("schedules-new:alpha"));
    const form = view.getByTestId("schedule-form:alpha");
    fireEvent.change(within(form).getByTestId("schedule-form-name"), { target: { value: "Queue" } });
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Check the queue" } });
    fireEvent.submit(form);

    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));
    expect(state.calls.find((call) => call.command === "createSchedule")?.params).toEqual({
      name: "Queue",
      content: "Check the queue",
      trigger: { type: "once", due_at_utc: new Date(2030, 0, 2, 9, 0, 0).toISOString() },
    });
    expect(present(view, "schedule-form:alpha")).toBe(false);
    expect(state.calls.map((call) => call.command)).toEqual(["listSchedules", "createSchedule", "listSchedules"]);
  });

  it("creates a daily schedule on chosen weekdays in a named time zone", async () => {
    const { run, state } = gateway();
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedules-empty:alpha")).toBe(true));

    fireEvent.click(view.getByTestId("schedules-new:alpha"));
    const form = view.getByTestId("schedule-form:alpha");
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Morning digest" } });
    fireEvent.change(within(form).getByTestId("schedule-form-timing"), { target: { value: "daily" } });
    fireEvent.change(within(form).getByTestId("schedule-form-time"), { target: { value: "07:30" } });
    fireEvent.click(within(form).getByTestId("schedule-form-day:5"));
    fireEvent.click(within(form).getByTestId("schedule-form-day:1"));
    fireEvent.change(within(form).getByTestId("schedule-form-timezone"), { target: { value: "Europe/Paris" } });
    fireEvent.submit(form);

    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));
    expect(state.calls.find((call) => call.command === "createSchedule")?.params).toEqual({
      content: "Morning digest",
      trigger: {
        type: "calendar",
        timezone: "Europe/Paris",
        minute: { kind: "values", values: [30] },
        hour: { kind: "values", values: [7] },
        day_of_month: { kind: "any" },
        month: { kind: "any" },
        day_of_week: { kind: "values", values: [1, 5] },
      },
    });
    expect(view.getByTestId("schedule-when:sch_1").textContent).toBe("Mon, Fri at 07:30 (Europe/Paris)");
  });

  it("edits, pauses, resumes and deletes a schedule", async () => {
    const { run, state } = gateway([item()]);
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));

    fireEvent.click(view.getByTestId("schedule-edit:sch_1"));
    const form = view.getByTestId("schedule-form:alpha");
    expect((within(form).getByTestId("schedule-form-timing") as HTMLSelectElement).value).toBe("keep");
    expect((within(form).getByTestId("schedule-form-content") as HTMLTextAreaElement).value).toBe("Post the standup summary");
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Post it now" } });
    fireEvent.submit(form);
    await waitFor(() => expect(view.getByTestId("schedule:sch_1").textContent).toContain("Post it now"));
    expect(present(view, "schedule-form:alpha")).toBe(false);

    fireEvent.click(view.getByTestId("schedule-pause:sch_1"));
    await waitFor(() => expect(view.getByTestId("schedule-phase:sch_1").textContent).toBe("Paused"));
    await waitFor(() => expect(present(view, "schedule-resume:sch_1")).toBe(true));
    fireEvent.click(view.getByTestId("schedule-resume:sch_1"));
    await waitFor(() => expect(view.getByTestId("schedule-phase:sch_1").textContent).toBe("Active"));

    await waitFor(() => expect((view.getByTestId("schedule-delete:sch_1") as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(view.getByTestId("schedule-delete:sch_1"));
    expect(state.calls.some((call) => call.command === "deleteSchedule")).toBe(false);
    fireEvent.click(view.getByTestId("schedule-delete-confirm:sch_1"));
    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(false));
    expect(present(view, "schedules-empty:alpha")).toBe(true);

    expect(state.calls.filter((call) => call.command !== "listSchedules")).toEqual([
      { command: "updateSchedule", params: { schedule_id: "sch_1", content: "Post it now" } },
      { command: "pauseSchedule", params: { schedule_id: "sch_1" } },
      { command: "resumeSchedule", params: { schedule_id: "sch_1" } },
      { command: "deleteSchedule", params: { schedule_id: "sch_1" } },
    ]);
  });

  it("shows a refusal by its kind and keeps the form for another try", async () => {
    const { run, state } = gateway();
    state.fail = (call) => (call.command === "createSchedule" ? rpcError(-32004, { kind: "governed_target" }) : null);
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedules-empty:alpha")).toBe(true));

    fireEvent.click(view.getByTestId("schedules-new:alpha"));
    const form = view.getByTestId("schedule-form:alpha");
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Check the queue" } });
    fireEvent.submit(form);

    await waitFor(() => expect(present(view, "schedules-error:alpha")).toBe(true));
    const refusal = view.getByTestId("schedules-error:alpha");
    expect(refusal.getAttribute("role")).toBe("alert");
    expect(refusal.textContent).toBe(
      "This agent admits only governed input, which needs you present, so a schedule cannot send to it.",
    );
    expect(present(view, "schedule-form:alpha")).toBe(true);
    expect((within(view.getByTestId("schedule-form:alpha")).getByTestId("schedule-form-content") as HTMLTextAreaElement).value)
      .toBe("Check the queue");
  });

  it("drops a schedule that is gone and closes its form", async () => {
    const { run, state } = gateway([item()]);
    state.fail = (call) => {
      if (call.command !== "updateSchedule") return null;
      state.items = [];
      return rpcError(-32001, { kind: "schedule_not_found" });
    };
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));

    fireEvent.click(view.getByTestId("schedule-edit:sch_1"));
    fireEvent.submit(view.getByTestId("schedule-form:alpha"));

    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(false));
    expect(view.getByTestId("schedules-error:alpha").textContent).toBe("This schedule no longer exists.");
    expect(present(view, "schedule-form:alpha")).toBe(false);
  });

  it("checks the form before it asks the gateway", async () => {
    const { run, state } = gateway();
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedules-empty:alpha")).toBe(true));

    fireEvent.click(view.getByTestId("schedules-new:alpha"));
    fireEvent.submit(view.getByTestId("schedule-form:alpha"));

    expect(view.getByTestId("schedule-form-error").textContent).toBe("Write the message to send.");
    expect(view.getByTestId("schedule-form-error").getAttribute("role")).toBe("alert");
    expect(state.calls.some((call) => call.command === "createSchedule")).toBe(false);
  });

  it("reads the list again on pane refreshes, at most every 15 seconds, and hides a list it cannot read", async () => {
    const { run, state } = gateway([item()]);
    let clock = NOW.getTime();
    const ticking = () => new Date(clock);
    const view = render(section(run, true, { refreshToken: 1, now: ticking }));
    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));

    view.rerender(section(run, true, { refreshToken: 2, now: ticking }));
    expect(state.calls.map((call) => call.command)).toEqual(["listSchedules"]);

    clock += 16_000;
    state.fail = (call) => (call.command === "listSchedules" ? rpcError(-32030, { kind: "access_denied" }) : null);
    view.rerender(section(run, true, { refreshToken: 3, now: ticking }));
    await waitFor(() => expect(present(view, "schedules-load-error:alpha")).toBe(true));
    expect(view.getByTestId("schedules-load-error:alpha").textContent).toBe("You may not view this agent's schedules.");
    expect(present(view, "schedule:sch_1")).toBe(false);

    clock += 16_000;
    state.fail = null;
    view.rerender(section(run, true, { refreshToken: 4, now: ticking }));
    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));
    expect(present(view, "schedules-load-error:alpha")).toBe(false);
    expect(state.calls.map((call) => call.command)).toEqual(["listSchedules", "listSchedules", "listSchedules"]);
  });

  it("names a change made meanwhile as a conflict and reads the list again", async () => {
    const { run, state } = gateway([item()]);
    state.fail = (call) =>
      call.command === "updateSchedule"
        ? rpcError(-32009, { kind: "schedule_revision_conflict", expected_revision: 1, actual_revision: 2 })
        : null;
    const view = render(section(run, true));
    await waitFor(() => expect(present(view, "schedule:sch_1")).toBe(true));

    fireEvent.click(view.getByTestId("schedule-edit:sch_1"));
    fireEvent.submit(view.getByTestId("schedule-form:alpha"));

    await waitFor(() => expect(present(view, "schedules-error:alpha")).toBe(true));
    expect(view.getByTestId("schedules-error:alpha").textContent).toBe(
      "This schedule changed meanwhile, so nothing was changed. Check it and try again.",
    );
    expect(present(view, "schedule-form:alpha")).toBe(true);
    await waitFor(() =>
      expect(state.calls.map((call) => call.command)).toEqual(["listSchedules", "updateSchedule", "listSchedules"]),
    );
  });
});
