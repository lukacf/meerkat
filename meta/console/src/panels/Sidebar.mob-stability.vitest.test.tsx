import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { consoleMobTree, resolveConsoleMobChoice } from "@console-core";
import { Sidebar } from "./Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";

// The roster does not re-group, re-sort or jump to its top when a mob or a
// member appears or goes away, and a stored choice that no longer exists is
// dropped instead of coming back later.

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
});

const agent = (id: string, label: string, mobId: string): ConsoleAgent => ({
  agent_id: id,
  member_id: id,
  identity: id,
  label,
  kind: "mob_agent",
  role: "analyst",
  mob_id: mobId,
});

const ONE_MOB: ConsoleMob[] = [{ mob_id: "ops-team", member_count: 2, state: "running" }];
const TWO_MOBS: ConsoleMob[] = [
  ...ONE_MOB,
  { mob_id: "research-team", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1, state: "running" },
];
const PRIMARY = [agent("planner", "Planner", "ops-team"), agent("dispatcher", "Dispatcher", "ops-team")];
const NESTED = [agent("research-team/reviewer", "Reviewer", "research-team")];
const props = {
  primaryMobId: "ops-team",
  selectedMemberId: "",
  recentActivity: [],
  collapsed: false,
  visibleControls: [],
  onSelect: vi.fn(),
  onOpenControl: vi.fn(),
  onSelectMob: vi.fn(),
  storageNamespace: "configuration-a",
  preferenceScope: "principal-a",
};
const VIEW_KEY = "meta-console-sidebar-view:principal-a";

const mobHeaders = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-testid^="sidebar-mob:"]')].map((node) => node.getAttribute("data-testid"));
const categoryHeaders = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-testid^="sidebar-section-toggle:"]')].map((node) => node.getAttribute("data-testid"));

describe("the roster's grouping", () => {
  it("stays by category when a second mob appears, and keeps its scroll position", () => {
    const view = render(<Sidebar {...props} mobs={ONE_MOB} agents={PRIMARY} />);
    expect(mobHeaders(view.container)).toEqual([]);
    const sections = categoryHeaders(view.container);
    expect(sections.length).toBeGreaterThan(0);
    const list = view.getByTestId("sidebar-agent-list");
    list.scrollTop = 120;
    view.rerender(<Sidebar {...props} mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    expect(mobHeaders(view.container)).toEqual([]);
    expect(list.scrollTop).toBe(120);
    expect(view.queryByTestId("sidebar-view-summary") === null).toBe(true);
    // The second mob goes away again: nothing moves.
    view.rerender(<Sidebar {...props} mobs={ONE_MOB} agents={PRIMARY} />);
    expect(categoryHeaders(view.container)).toEqual(sections);
    expect(list.scrollTop).toBe(120);
    // Mob is offered once several mobs are listed, and the person can pick it.
    view.rerender(<Sidebar {...props} mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    fireEvent.click(view.getByTestId("sidebar-view-group:mob"));
    expect(mobHeaders(view.container)).toEqual(["sidebar-mob:ops-team", "sidebar-mob:research-team"]);
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toMatchObject({ group_by: "mob" });
  });

  it("stays by mob when the console is left with one mob", () => {
    const view = render(<Sidebar {...props} mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    expect(mobHeaders(view.container)).toEqual(["sidebar-mob:ops-team", "sidebar-mob:research-team"]);
    const list = view.getByTestId("sidebar-agent-list");
    list.scrollTop = 80;
    view.rerender(<Sidebar {...props} mobs={ONE_MOB} agents={PRIMARY} />);
    expect(mobHeaders(view.container)).toEqual(["sidebar-mob:ops-team"]);
    expect(categoryHeaders(view.container)).toEqual([]);
    expect(list.scrollTop).toBe(80);
    expect(localStorage.getItem(VIEW_KEY) === null).toBe(true);
  });

  it("is decided by the first mob list the roster is given, not by the empty roster before it", () => {
    const view = render(<Sidebar {...props} mobsLoaded={false} agents={[]} />);
    expect(mobHeaders(view.container)).toEqual([]);
    view.rerender(<Sidebar {...props} mobsLoaded mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    expect(mobHeaders(view.container)).toEqual(["sidebar-mob:ops-team", "sidebar-mob:research-team"]);
    expect(localStorage.getItem(VIEW_KEY) === null).toBe(true);
  });

  it("drops a stored Mob grouping once the console is seen to list one mob", () => {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: "mob", sort_by: "name" }));
    const view = render(<Sidebar {...props} mobs={ONE_MOB} agents={PRIMARY} />);
    expect(mobHeaders(view.container)).toEqual([]);
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toMatchObject({ group_by: "category", sort_by: "name" });
    // A second mob that appears later does not bring the grouping back.
    view.rerender(<Sidebar {...props} mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    expect(mobHeaders(view.container)).toEqual([]);
  });

  it("does not read a grouping kept from before a second mob was listed as a changed view", () => {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: "category", sort_by: "manual" }));
    const view = render(<Sidebar {...props} mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    expect(mobHeaders(view.container)).toEqual([]);
    expect(view.queryByTestId("sidebar-view-summary") === null).toBe(true);
    expect(view.getByTestId("sidebar-view-menu").getAttribute("data-customized") === null).toBe(true);
  });

  it("keeps the person's grouping when the view is reset", () => {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: "category", sort_by: "name" }));
    const view = render(<Sidebar {...props} mobs={TWO_MOBS} agents={[...PRIMARY, ...NESTED]} />);
    expect(view.getByTestId("sidebar-view-summary").textContent).toBe(" · By name");
    fireEvent.click(view.getByTestId("sidebar-view-reset"));
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toEqual({ v: 1, group_by: "category", sort_by: "manual" });
    expect(mobHeaders(view.container)).toEqual([]);
  });
});

describe("the mob choice", () => {
  it("does not filter a roster that lists one mob", () => {
    const view = render(<Sidebar {...props} mobs={ONE_MOB} agents={PRIMARY} selectedMobId="ops-team" />);
    expect(view.getByTestId("sidebar-agent-count").textContent).toBe("2 agents");
    expect(view.queryByTestId("sidebar-view-summary") === null).toBe(true);
  });

  it("applies among several listed mobs, rests with one, and is dropped once a loaded list does not hold it", () => {
    const several = consoleMobTree(TWO_MOBS, "ops-team");
    const single = consoleMobTree(ONE_MOB, "ops-team");
    const chosen = resolveConsoleMobChoice(several, "research-team", true);
    expect([chosen.mobId, [...(chosen.mobIds || [])], chosen.drop]).toEqual(["research-team", ["research-team"], false]);
    // One mob listed, and it is the stored one: nothing to choose, nothing dropped.
    expect(resolveConsoleMobChoice(single, "ops-team", true)).toEqual({ mobId: null, mobIds: null, drop: false });
    // The stored mob is gone from a loaded list: all mobs, and the choice is dropped.
    expect(resolveConsoleMobChoice(single, "research-team", true)).toEqual({ mobId: null, mobIds: null, drop: true });
    expect(resolveConsoleMobChoice(several, "retired-team", true)).toEqual({ mobId: null, mobIds: null, drop: true });
    // Before any list has loaded nothing is dropped.
    expect(resolveConsoleMobChoice([], "research-team", false)).toEqual({ mobId: null, mobIds: null, drop: false });
    expect(resolveConsoleMobChoice(several, null, true)).toEqual({ mobId: null, mobIds: null, drop: false });
  });
});
