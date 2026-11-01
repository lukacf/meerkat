import { act, fireEvent, render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { Agenda, type AgendaRow } from "./agenda";

// The Schedules agenda (console v2 spec B10). DOM assertions compare
// booleans and strings, never elements.

const HOUR = 3_600_000;
const START = Date.UTC(2026, 4, 4, 8, 0, 0);
const END = START + 24 * HOUR;

const rows: AgendaRow[] = [
  {
    key: "coordinator",
    label: <span>Coordinator</span>,
    labelText: "Coordinator",
    runs: [
      { id: "sch_summary:0", t: START + 2 * HOUR, shape: "diamond", next: true, label: "Monday summary · Mon 12:00 Europe/Paris" },
      { id: "sch_once:0", t: START + 5 * HOUR, shape: "square", next: true, label: "Release note · Mon 15:00" },
    ],
  },
  {
    key: "support",
    label: <span>Support</span>,
    labelText: "Support",
    runs: [
      { id: "sch_sweep:0", t: START + 1 * HOUR, shape: "tick", next: true, label: "Inbox sweep · Mon 11:00" },
      { id: "sch_sweep:1", t: START + 3 * HOUR, shape: "tick", label: "Inbox sweep · Mon 13:00" },
      { id: "sch_paused:0", t: START + 4 * HOUR, shape: "diamond", paused: true, label: "Weekly review (paused) · Mon 14:00" },
    ],
    bands: [{ id: "sch_pulse:band", from: START, to: END, words: "every 5 min", label: "Pulse · every 5 min, 288 runs in the next 24 hours" }],
  },
  { key: "monitor", label: <span>Health Monitor</span>, labelText: "Health Monitor", runs: [] },
];

function agenda(extra: Partial<Parameters<typeof Agenda>[0]> = {}) {
  return (
    <Agenda
      rows={rows}
      start={START}
      end={END}
      label="6 runs in the next 24 hours across 3 agents"
      testIdPrefix="schedules-agenda-run"
      rowTestIdPrefix="schedules-agenda-row"
      testId="schedules-agenda"
      {...extra}
    />
  );
}

describe("Agenda", () => {
  it("draws one row per agent and one mark per run, shaped by kind", () => {
    const view = render(agenda());
    expect(view.getByTestId("schedules-agenda").getAttribute("aria-label")).toBe("6 runs in the next 24 hours across 3 agents");
    expect(view.queryByTestId("schedules-agenda-row:monitor") !== null).toBe(true);
    const once = view.getByTestId("schedules-agenda-run:sch_once:0");
    expect(once.getAttribute("data-shape")).toBe("square");
    expect(once.getAttribute("aria-label")).toBe("Release note · Mon 15:00");
    expect(view.getByTestId("schedules-agenda-run:sch_summary:0").getAttribute("data-shape")).toBe("diamond");
    expect(view.getByTestId("schedules-agenda-run:sch_sweep:1").getAttribute("data-shape")).toBe("tick");
    // The next run is the accent; the rest are ink.
    expect(view.getByTestId("schedules-agenda-run:sch_sweep:0").classList.contains("is-live")).toBe(true);
    expect(view.getByTestId("schedules-agenda-run:sch_sweep:1").classList.contains("is-live")).toBe(false);
    // A paused schedule's run is hollow and never the accent.
    const paused = view.getByTestId("schedules-agenda-run:sch_paused:0");
    expect(paused.getAttribute("data-paused")).toBe("true");
    expect(paused.classList.contains("is-live")).toBe(false);
    // A dense repeat is one band with its words.
    expect(view.getByTestId("schedules-agenda-run:sch_pulse:band").getAttribute("aria-label")).toBe("Pulse · every 5 min, 288 runs in the next 24 hours");
    expect(view.container.textContent?.includes("every 5 min")).toBe(true);
  });

  it("labels the axis from now and lists every run in a table twin", () => {
    const view = render(agenda());
    const axis = view.container.querySelector(".cc-agenda__row--axis");
    expect(axis?.textContent?.startsWith("now")).toBe(true);
    expect(axis?.textContent?.includes("+12h")).toBe(true);
    const table = view.container.querySelector("table.cc-visually-hidden");
    expect(table?.querySelectorAll("tbody tr").length).toBe(6);
    expect(table?.textContent?.includes("Inbox sweep · Mon 11:00")).toBe(true);
    expect(table?.textContent?.includes("calendar, next")).toBe(true);
  });

  it("is one tab stop that the arrow keys move along and between rows", () => {
    const view = render(agenda());
    const focusable = Array.from(view.container.querySelectorAll("[data-agenda-key]")).filter((mark) => mark.getAttribute("tabindex") === "0");
    expect(focusable.length).toBe(1);
    const first = view.getByTestId("schedules-agenda-run:sch_summary:0");
    expect(first.getAttribute("tabindex")).toBe("0");
    act(() => first.focus());
    fireEvent.keyDown(first, { key: "ArrowRight" });
    const second = view.getByTestId("schedules-agenda-run:sch_once:0");
    expect(document.activeElement === second).toBe(true);
    expect(second.getAttribute("tabindex")).toBe("0");
    expect(first.getAttribute("tabindex")).toBe("-1");
    // Down: the nearest run in time on the next row with runs.
    fireEvent.keyDown(second, { key: "ArrowDown" });
    expect(document.activeElement === view.getByTestId("schedules-agenda-run:sch_paused:0")).toBe(true);
    // Focus shows the mark's words.
    expect(view.container.querySelector(".cc-chart-tip")?.textContent).toBe("Weekly review (paused) · Mon 14:00");
    fireEvent.keyDown(document.activeElement as Element, { key: "Home" });
    expect(document.activeElement === view.getByTestId("schedules-agenda-run:sch_pulse:band")).toBe(true);
  });

  it("selects a run with Enter and hands back its row", () => {
    const onSelect = vi.fn();
    const view = render(agenda({ onSelect }));
    const mark = view.getByTestId("schedules-agenda-run:sch_summary:0");
    expect(mark.getAttribute("role")).toBe("button");
    act(() => mark.focus());
    fireEvent.keyDown(mark, { key: "Enter" });
    expect(onSelect.mock.calls[0]?.[0]?.id).toBe("sch_summary:0");
    expect(onSelect.mock.calls[0]?.[1]?.key).toBe("coordinator");
  });

  it("leaves out runs outside the window", () => {
    const view = render(agenda({ end: START + 2 * HOUR }));
    expect(view.queryByTestId("schedules-agenda-run:sch_once:0") === null).toBe(true);
    expect(view.queryByTestId("schedules-agenda-run:sch_summary:0") !== null).toBe(true);
  });
});
