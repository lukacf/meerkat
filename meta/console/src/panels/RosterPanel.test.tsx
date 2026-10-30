import { fireEvent, render, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ConsoleAgent } from "../types";
import { RosterPanel, rosterActivity, rosterDotTone, rosterHealthChip } from "./RosterPanel";
import { AgentDirectoryProvider, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";

// DOM assertions compare booleans and strings, never elements.

const NOW = Date.now();

function agent(overrides: Partial<ConsoleAgent> & { member_id: string; label: string }): ConsoleAgent {
  return {
    agent_id: overrides.member_id,
    identity: overrides.member_id,
    kind: "identity",
    state: "active",
    ...overrides,
  } as ConsoleAgent;
}

const coordinator = agent({
  member_id: "identity:coordinator",
  label: "Coordinator",
  role: "coordinator",
  session_id: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D",
  generation: 4,
  wired_to: ["identity:analyst", "identity:unknown-peer"],
  progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 2 * 60_000, last_progress_event: "execution_advanced", health: "healthy" },
  affordances: { can_respawn: true, can_retire: true },
});
const analyst = agent({
  member_id: "identity:analyst",
  label: "Research Analyst",
  role: "analyst",
  response_phase: "tool-executing",
  progress: { run_state: "run_open", in_flight_work: 2, last_progress_at_ms: NOW - 40_000, last_progress_event: "execution_advanced", health: "healthy" },
});
const writer = agent({
  member_id: "identity:writer",
  label: "Docs Writer",
  role: "writer",
  progress: { run_state: "run_open", in_flight_work: 1, last_progress_at_ms: NOW - 47 * 60_000, last_progress_event: "execution_advanced", health: "wedged" },
});
const monitor = agent({ member_id: "identity:monitor", label: "Nightly Monitor", role: "monitor", state: "retired", group: "internal" });

function renderRoster(agents: ConsoleAgent[], extra: Partial<Parameters<typeof RosterPanel>[0]> = {}) {
  const handlers = {
    onSelect: vi.fn(),
    onChat: vi.fn(),
    onDetails: vi.fn(),
    onLifecycle: vi.fn(),
  };
  const directory = createAgentDirectory(agents.map((entry) => ({ id: entry.identity || entry.member_id, label: entry.label })));
  const view = render(
    <AgentDirectoryProvider directory={directory}>
      <RosterPanel agents={agents} canResetLifecycle {...handlers} {...extra} />
    </AgentDirectoryProvider>,
  );
  return { view, ...handlers };
}

describe("Roster words", () => {
  it("says what an agent is doing instead of raw run states", () => {
    expect(rosterActivity(coordinator)).toBe("working");
    expect(rosterActivity(analyst)).toBe("running a tool");
    expect(rosterActivity({ ...analyst, response_phase: "generating" })).toBe("writing a reply");
    expect(rosterActivity({ ...coordinator, progress: { ...coordinator.progress!, in_flight_work: 3 } })).toBe("working on 3 tasks");
    expect(rosterActivity({ ...coordinator, progress: { ...coordinator.progress!, run_state: "idle", in_flight_work: 0 } })).toBe("idle");
    expect(rosterActivity(monitor)).toBe("retired");
  });

  it("raises a chip only for health problems and never tones status with the accent", () => {
    expect(rosterHealthChip(coordinator)).toBe(null);
    expect(rosterHealthChip(writer)).toEqual({ label: "Stalled", tone: "crit" });
    expect(rosterHealthChip({ ...coordinator, progress: { ...coordinator.progress!, health: "degraded" } })).toEqual({ label: "Degraded", tone: "warn" });
    expect(rosterHealthChip(monitor)).toBe(null);
    expect(rosterDotTone(coordinator)).toBe("ok");
    expect(rosterDotTone(writer)).toBe("crit");
    expect(rosterDotTone(monitor)).toBe("neutral");
  });
});

describe("RosterPanel", () => {
  it("lists agents by name with role and activity, and keeps keys out of the rows", () => {
    const { view } = renderRoster([coordinator, analyst, writer, monitor]);
    const row = view.getByTestId("roster-row:identity:analyst");
    expect(row.tagName).toBe("BUTTON");
    expect(row.textContent?.includes("Research Analyst")).toBe(true);
    expect(row.textContent?.includes("Analyst · running a tool")).toBe(true);
    expect(row.textContent?.includes("identity:analyst")).toBe(false);
    expect(row.querySelector(".cc-row__title")?.getAttribute("title")).toBe("identity:analyst");
    expect(view.getByTestId("roster-row:identity:writer").textContent?.includes("Stalled")).toBe(true);
    expect(view.getByTestId("roster-row:identity:writer").getAttribute("data-health")).toBe("wedged");
    expect(view.getByText("4 agents · 3 active · 1 needs attention").className).toBe("cc-panel-head__sub");
  });

  it("selects with the keyboard and opens the detail with words first and IDs collapsed", () => {
    const { view, onSelect } = renderRoster([coordinator, analyst]);
    const row = view.getByTestId("roster-row:identity:coordinator");
    fireEvent.click(row);
    expect(onSelect.mock.calls[0]?.[0]?.member_id).toBe("identity:coordinator");
    expect(row.getAttribute("aria-current")).toBe("true");
    const split = view.container.querySelector(".cc-split");
    expect(split?.classList.contains("has-detail")).toBe(true);

    const detail = view.getByTestId("roster-detail:identity:coordinator");
    expect(within(detail).getByRole("heading", { name: "Coordinator" }).tagName).toBe("H3");
    expect(detail.textContent?.includes("Working")).toBe(true);
    expect(detail.textContent?.includes("1 task")).toBe(true);
    const tech = detail.querySelector("details.cc-tech") as HTMLDetailsElement | null;
    expect(tech !== null && tech.open === false).toBe(true);
    expect(tech?.textContent?.includes("session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D")).toBe(true);
    expect(within(detail).getByRole("button", { name: "Copy session" }).classList.contains("cc-copy-btn")).toBe(true);

    fireEvent.click(view.getByRole("button", { name: "All agents" }));
    expect(split?.classList.contains("has-detail")).toBe(false);
  });

  it("shows peers by name and selects a known peer", () => {
    const { view, onSelect } = renderRoster([coordinator, analyst]);
    fireEvent.click(view.getByTestId("roster-row:identity:coordinator"));
    const detail = view.getByTestId("roster-detail:identity:coordinator");
    expect(detail.textContent?.includes("unknown-peer")).toBe(true);
    expect(detail.textContent?.includes("identity:unknown-peer")).toBe(false);
    const peer = within(detail).getByRole("button", { name: "Research Analyst" });
    expect(peer.getAttribute("title")).toBe("identity:analyst");
    fireEvent.click(peer);
    expect(onSelect.mock.calls.at(-1)?.[0]?.member_id).toBe("identity:analyst");
  });

  it("offers chat first and groups lifecycle actions, Retire as danger", () => {
    const { view, onChat, onLifecycle } = renderRoster([coordinator]);
    const detail = view.getByTestId("roster-detail:identity:coordinator");
    fireEvent.click(within(detail).getByRole("button", { name: "Open chat" }));
    expect(onChat.mock.calls.length).toBe(1);
    expect(within(detail).getByRole("button", { name: "Open chat" }).classList.contains("cc-btn--primary")).toBe(true);
    const retire = within(detail).getByRole("button", { name: "Retire" });
    expect(retire.classList.contains("cc-btn--danger")).toBe(true);
    fireEvent.click(retire);
    expect(onLifecycle.mock.calls[0]).toEqual(["identity:coordinator", "meta/retire"]);
    expect(within(detail).getByRole("group", { name: "Lifecycle" }).textContent).toBe("RespawnResetRetire");
  });

  it("filters by the sidebar's groups and explains an empty result with a Clear action", () => {
    const { view } = renderRoster([coordinator, analyst, monitor]);
    const groups = view.getByRole("group", { name: "Agent groups" });
    expect(within(groups).getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(within(groups).getByRole("button", { name: "Coordinators" }));
    expect(view.queryByTestId("roster-row:identity:analyst") === null).toBe(true);
    expect(view.queryByTestId("roster-row:identity:coordinator") !== null).toBe(true);

    fireEvent.change(view.getByRole("searchbox", { name: "Search agents" }), { target: { value: "zzz" } });
    expect(view.getByText('No agents match "zzz"').className).toBe("cc-panel-empty__title");
    fireEvent.click(view.getByRole("button", { name: "Clear" }));
    expect(view.queryByTestId("roster-row:identity:analyst") !== null).toBe(true);
  });

  it("has a designed empty state", () => {
    const { view } = renderRoster([]);
    expect(view.getByText("No agents yet").className).toBe("cc-panel-empty__title");
  });
});
