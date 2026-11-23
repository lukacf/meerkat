import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { Sidebar } from "./Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";

// The mob choice under every grouping: the person can see it, change it and
// leave it from where they are, a search says which mob it looked in, and a
// manual order made while one mob shows cannot move agents out of sight.

const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 2000 });
});
afterAll(() => {
  if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
});

const agent = (id: string, label: string, mobId: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  agent_id: id,
  member_id: id,
  identity: id,
  label,
  kind: "mob_agent",
  role: "analyst",
  mob_id: mobId,
  ...extra,
});

const MOBS: ConsoleMob[] = [
  { mob_id: "ops-team", member_count: 2, state: "running" },
  { mob_id: "research-team", parent_mob_id: "ops-team", host_identity: "planner", member_count: 2, state: "running" },
  { mob_id: "reading-room", parent_mob_id: "research-team", member_count: 1, state: "running" },
];
const AGENTS: ConsoleAgent[] = [
  agent("planner", "Planner", "ops-team", { role: "coordinator" }),
  agent("dispatcher", "Dispatcher", "ops-team"),
  agent("research-team/reviewer", "Reviewer", "research-team"),
  agent("research-team/scout", "Scout", "research-team"),
  agent("reading-room/reader", "Reader", "reading-room"),
];
const onSelectMob = vi.fn();
const props = {
  agents: AGENTS,
  mobs: MOBS,
  primaryMobId: "ops-team",
  selectedMemberId: "",
  recentActivity: [],
  collapsed: false,
  visibleControls: [],
  onSelect: vi.fn(),
  onOpenControl: vi.fn(),
  onSelectMob,
  storageNamespace: "configuration-a",
  preferenceScope: "principal-a",
};
const VIEW_KEY = "meta-console-sidebar-view:principal-a";
const ORDER_KEY = "meta-console-sidebar-agent-order:principal-a";

const agentRows = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-testid^="sidebar-agent:"]')].map((node) => node.getAttribute("data-testid"));

describe("a search under a mob choice", () => {
  it("says which mob it looked in and how many matches the other mobs hold", () => {
    const view = render(<Sidebar {...props} selectedMobId="research-team" />);
    fireEvent.change(view.getByTestId("sidebar-search"), { target: { value: "dispatch" } });
    const empty = view.getByTestId("sidebar-empty-filtered");
    expect(empty.querySelector("span")?.textContent).toBe("No matching agents in Planner's research team. 1 in other mobs.");
    fireEvent.click(view.getByTestId("sidebar-search-all-mobs"));
    expect(onSelectMob).toHaveBeenLastCalledWith(null);
    // The host applies the choice: the search stays and now finds the agent.
    view.rerender(<Sidebar {...props} selectedMobId={null} />);
    expect(agentRows(view.container)).toEqual(["sidebar-agent:dispatcher"]);
  });

  it("clears the mob choice with the other filters", () => {
    const view = render(<Sidebar {...props} selectedMobId="research-team" />);
    fireEvent.change(view.getByTestId("sidebar-search"), { target: { value: "no such agent" } });
    expect(view.getByTestId("sidebar-empty-filtered").querySelector("span")?.textContent)
      .toBe("No matching agents in Planner's research team.");
    fireEvent.click(view.getByTestId("sidebar-clear-filters"));
    expect(onSelectMob).toHaveBeenLastCalledWith(null);
    expect((view.getByTestId("sidebar-search") as HTMLInputElement).value).toBe("");
  });

  it("finds agents of the chosen mob and of the mobs nested under it only", () => {
    const view = render(<Sidebar {...props} selectedMobId="research-team" />);
    fireEvent.change(view.getByTestId("sidebar-search"), { target: { value: "re" } });
    expect(agentRows(view.container)).toEqual(["sidebar-agent:research-team/reviewer", "sidebar-agent:research-team/scout", "sidebar-agent:reading-room/reader"]);
    expect(view.queryByTestId("sidebar-empty-filtered") === null).toBe(true);
  });

  it("says so when the chosen mob lists no agent", () => {
    const view = render(<Sidebar {...props} agents={AGENTS.slice(0, 2)} selectedMobId="research-team" />);
    expect(view.getByTestId("sidebar-empty-filtered").querySelector("span")?.textContent).toBe("No agents in Planner's research team.");
    expect(view.getByTestId("sidebar-search-all-mobs").textContent).toBe("Show all mobs");
    expect(view.queryByTestId("sidebar-clear-filters") === null).toBe(true);
  });
});

describe("the mob choice in the roster's view options", () => {
  for (const groupBy of ["category", "none", "mob"] as const) {
    it(`is shown and changed there when the roster is grouped by ${groupBy}`, () => {
      localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: groupBy, sort_by: "manual" }));
      const view = render(<Sidebar {...props} selectedMobId="research-team" />);
      expect(agentRows(view.container).sort()).toEqual([
        "sidebar-agent:reading-room/reader",
        "sidebar-agent:research-team/reviewer",
        "sidebar-agent:research-team/scout",
      ]);
      fireEvent.click(view.getByTestId("sidebar-view-menu"));
      const select = view.getByTestId("sidebar-view-mob") as HTMLSelectElement;
      expect(select.value).toBe("research-team");
      expect([...select.options].map((option) => [option.value, option.textContent?.trim()])).toEqual([
        ["", "All mobs"],
        ["ops-team", "Ops team"],
        ["research-team", "Planner's research team"],
        ["reading-room", "Reading room team"],
      ]);
      fireEvent.change(select, { target: { value: "reading-room" } });
      expect(onSelectMob).toHaveBeenLastCalledWith("reading-room");
      fireEvent.change(select, { target: { value: "" } });
      expect(onSelectMob).toHaveBeenLastCalledWith(null);
    });
  }

  it("is not offered on a console that lists one mob", () => {
    const view = render(<Sidebar {...props} mobs={MOBS.slice(0, 1)} agents={AGENTS.slice(0, 2)} />);
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    expect(view.queryByTestId("sidebar-view-mob") === null).toBe(true);
  });
});

describe("a manual order under a mob choice", () => {
  it("cannot be changed while one mob shows, and says why", () => {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: "none", sort_by: "manual" }));
    const view = render(<Sidebar {...props} selectedMobId="research-team" />);
    expect(view.container.querySelector('[data-testid^="sidebar-agent-grip:"]') === null).toBe(true);
    fireEvent.keyDown(view.getByTestId("sidebar-agent:research-team/reviewer"), { key: "ArrowDown", altKey: true });
    expect(view.getByTestId("sidebar-reorder-live").textContent).toBe("Show all mobs to reorder agents.");
    expect(localStorage.getItem(ORDER_KEY) === null).toBe(true);
    // With all mobs shown the same move is made.
    view.rerender(<Sidebar {...props} selectedMobId={null} />);
    expect(view.container.querySelector('[data-testid^="sidebar-agent-grip:"]') !== null).toBe(true);
  });
});
