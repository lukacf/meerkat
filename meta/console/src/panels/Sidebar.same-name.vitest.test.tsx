import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Sidebar } from "./Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";

// Members with one name in several mobs: the console's own mob names its
// member `reviewer`; the mobs `alpha` and `beta` name theirs
// `alpha/reviewer` and `beta/reviewer`. Each is its own agent everywhere:
// nothing is keyed, matched or nested by the bare name.

afterEach(() => {
  cleanup();
  localStorage.clear();
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
  { mob_id: "alpha", parent_mob_id: "ops-team", host_identity: "planner", member_count: 2, state: "running" },
  { mob_id: "beta", parent_mob_id: "ops-team", host_identity: "planner", member_count: 2, state: "running" },
];
const AGENTS: ConsoleAgent[] = [
  agent("planner", "Planner", "ops-team", { role: "coordinator" }),
  agent("reviewer", "Reviewer", "ops-team"),
  agent("alpha/reviewer", "Reviewer", "alpha", { labels: { delegate_host_identity: "planner" } }),
  agent("beta/reviewer", "Reviewer", "beta", { labels: { delegate_host_identity: "planner", console_inbox_count: "3" } }),
  // Started by alpha's reviewer, and wired to beta's: each names its peer by
  // the peer's identity.
  agent("alpha/helper-1", "Helper", "alpha", { role: "helper", labels: { spawned_by: "alpha/reviewer" } }),
  agent("beta/worker-1", "Worker", "beta", { role: "worker", wired_to: ["beta/reviewer"] }),
];
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
  onTogglePinnedAgent: vi.fn(),
  storageNamespace: "configuration-a",
  preferenceScope: "principal-a",
};
const ORDER_KEY = "meta-console-sidebar-agent-order:principal-a";
const VIEW_KEY = "meta-console-sidebar-view:principal-a";

const listing = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-testid^="sidebar-mob:"], [data-testid^="sidebar-agent:"]')]
    .map((node) => node.getAttribute("data-testid") || "");
const agentRows = (container: HTMLElement) =>
  [...container.querySelectorAll<HTMLElement>('[data-testid^="sidebar-agent:"]')];

describe("members with one name in several mobs", () => {
  it("lists each in its own mob, under the agent that started it", () => {
    const view = render(<Sidebar {...props} />);
    expect(listing(view.container)).toEqual([
      "sidebar-mob:ops-team",
      "sidebar-agent:planner",
      "sidebar-agent:reviewer",
      "sidebar-mob:alpha",
      "sidebar-agent:alpha/reviewer",
      "sidebar-agent:alpha/helper-1",
      "sidebar-mob:beta",
      "sidebar-agent:beta/reviewer",
      "sidebar-agent:beta/worker-1",
    ]);
    // The helper nests under alpha's reviewer and the worker under beta's:
    // neither under the primary mob's reviewer.
    expect(view.getByTestId("sidebar-agent:alpha/helper-1").getAttribute("data-child-of-host")).toBe("true");
    expect(view.getByTestId("sidebar-agent:beta/worker-1").getAttribute("data-child-of-host")).toBe("true");
    expect(view.getByTestId("sidebar-agent:reviewer").getAttribute("data-child-of-host") === null).toBe(true);
  });

  it("nests by the peer's identity in the category view and the flat list too", () => {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: "none", sort_by: "manual" }));
    const view = render(<Sidebar {...props} />);
    const order = agentRows(view.container).map((node) => node.getAttribute("data-testid"));
    expect(order.indexOf("sidebar-agent:alpha/helper-1")).toBe(order.indexOf("sidebar-agent:alpha/reviewer") + 1);
    expect(order.indexOf("sidebar-agent:beta/worker-1")).toBe(order.indexOf("sidebar-agent:beta/reviewer") + 1);
  });

  it("selects, marks unread and pins each on its own", () => {
    const view = render(<Sidebar {...props} selectedMemberId="alpha/reviewer" pinnedAgentIds={new Set(["beta/reviewer"])} />);
    const active = agentRows(view.container).filter((node) => node.classList.contains("is-active"));
    expect(active.map((node) => node.getAttribute("data-testid"))).toEqual(["sidebar-agent:alpha/reviewer"]);
    const unread = agentRows(view.container).filter((node) => node.querySelector(".agent__inbox") !== null);
    expect(unread.map((node) => node.getAttribute("data-testid"))).toEqual(["sidebar-agent:beta/reviewer"]);
    const pinned = [...view.container.querySelectorAll('[data-testid^="sidebar-agent-pin:"][data-active="true"]')]
      .map((node) => node.getAttribute("data-testid"));
    // The pinned agent and the worker listed under it; no other reviewer.
    expect(pinned).toEqual(["sidebar-agent-pin:beta/reviewer"]);
    fireEvent.click(view.getByTestId("sidebar-agent:reviewer"));
    expect(props.onSelect.mock.lastCall?.[0]).toMatchObject({ member_id: "reviewer", identity: "reviewer" });
    fireEvent.click(view.getByTestId("sidebar-agent:beta/reviewer"));
    expect(props.onSelect.mock.lastCall?.[0]).toMatchObject({ member_id: "beta/reviewer", identity: "beta/reviewer" });
  });

  it("keeps a manual order by identity, so the three reviewers order apart", () => {
    localStorage.setItem(VIEW_KEY, JSON.stringify({ v: 1, group_by: "none", sort_by: "manual" }));
    localStorage.setItem(ORDER_KEY, JSON.stringify(["beta/reviewer", "reviewer", "alpha/reviewer", "planner"]));
    const view = render(<Sidebar {...props} />);
    const reviewers = agentRows(view.container)
      .map((node) => node.getAttribute("data-testid") || "")
      .filter((id) => id.endsWith("reviewer"));
    expect(reviewers).toEqual(["sidebar-agent:beta/reviewer", "sidebar-agent:reviewer", "sidebar-agent:alpha/reviewer"]);
    // The order key of a row is its identity, as it is written to storage.
    expect(view.getByTestId("sidebar-agent:alpha/reviewer").getAttribute("data-sidebar-order-id")).toBe("alpha/reviewer");
  });

  it("finds all three by the name they share, each row told apart by its mob header", () => {
    const view = render(<Sidebar {...props} />);
    fireEvent.change(view.getByTestId("sidebar-search"), { target: { value: "reviewer" } });
    expect(listing(view.container)).toEqual([
      "sidebar-mob:ops-team",
      "sidebar-agent:reviewer",
      "sidebar-mob:alpha",
      "sidebar-agent:alpha/reviewer",
      "sidebar-mob:beta",
      "sidebar-agent:beta/reviewer",
    ]);
  });
});
