import { cleanup, fireEvent, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { Sidebar } from "./Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";

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

// The primary mob, a mob nested two levels under it, and a second top-level
// mob no member owns, one of whose members is dormant.
const MOBS: ConsoleMob[] = [
  { mob_id: "ops-team", member_count: 2, state: "running" },
  { mob_id: "research", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1, state: "running" },
  { mob_id: "reading-room", parent_mob_id: "research", host_identity: "researcher", member_count: 1, state: "running" },
  { mob_id: "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f", member_count: 2, state: "stopped" },
];
const AGENTS: ConsoleAgent[] = [
  agent("planner", "Planner", "ops-team"),
  agent("dispatcher", "Dispatcher", "ops-team"),
  agent("researcher", "Researcher", "research", {
    labels: { source_mob_id: "research", delegate_host_identity: "planner", group: "Coordinators" },
  }),
  agent("reader", "Reader", "reading-room", {
    labels: { source_mob_id: "reading-room", delegate_host_identity: "researcher", group: "Coordinators" },
  }),
  agent("archivist", "Archivist", "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f", {
    labels: { source_mob_id: "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f" },
  }),
  agent("curator", "Curator", "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f", {
    state: "dormant",
    labels: { source_mob_id: "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f" },
  }),
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
  storageNamespace: "configuration-a",
  preferenceScope: "principal-a",
};
const VIEW_KEY = "meta-console-sidebar-view:principal-a";

/** The roster's mob headers and agent rows, in the order they are listed. */
const listing = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-testid^="sidebar-mob:"], [data-testid^="sidebar-agent:"]')]
    .map((node) => node.getAttribute("data-testid") || "");
const mobHeaders = (container: HTMLElement) =>
  [...container.querySelectorAll('[data-testid^="sidebar-mob:"]')]
    .map((node) => [node.getAttribute("data-testid")?.slice("sidebar-mob:".length), node.getAttribute("data-depth")]);

describe("Roster grouped by mob", () => {
  it("starts grouped by mob when the console lists several, as a tree that follows the nesting", () => {
    const view = render(<Sidebar {...props} />);
    expect(mobHeaders(view.container)).toEqual([
      ["ops-team", "0"],
      ["research", "1"],
      ["reading-room", "2"],
      ["6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f", "0"],
    ]);
    expect(listing(view.container)).toEqual([
      "sidebar-mob:ops-team",
      "sidebar-agent:dispatcher",
      "sidebar-agent:planner",
      "sidebar-mob:research",
      "sidebar-agent:researcher",
      "sidebar-mob:reading-room",
      "sidebar-agent:reader",
      "sidebar-mob:6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f",
      "sidebar-agent:archivist",
      "sidebar-agent:curator",
    ]);
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    expect(view.getByTestId("sidebar-view-group:mob").getAttribute("aria-pressed")).toBe("true");
  });

  it("names each mob in words with its count and status, the id only in its tooltip", () => {
    const view = render(<Sidebar {...props} />);
    const research = view.getByTestId("sidebar-mob:research");
    expect(within(research).getByTestId("sidebar-mob-toggle:research").textContent).toContain("Planner's research team");
    expect(within(research).getByTestId("sidebar-mob-count:research").textContent).toBe("1");
    const generated = view.getByTestId("sidebar-mob:6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f");
    const toggle = within(generated).getByTestId("sidebar-mob-toggle:6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f");
    expect(toggle.querySelector(".sidebar__mob-label")?.textContent).toBe("Team");
    expect(toggle.textContent).not.toContain("6f1c2d3e-4a5b");
    expect(toggle.textContent).not.toContain("2d3e4f");
    expect(toggle.getAttribute("title")).toContain("6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f");
    expect(toggle.getAttribute("title")).toContain("Stopped");
    expect(within(generated).getByTestId("sidebar-mob-count:6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f").textContent).toBe("2");
    expect(view.getByTestId("sidebar-mob-toggle:research").getAttribute("title")).toContain("Created by Planner");
  });

  it("collapses a mob with the mobs nested under it, and keeps that per configuration", () => {
    const first = render(<Sidebar {...props} />);
    fireEvent.click(first.getByTestId("sidebar-mob-toggle:research"));
    expect(first.getByTestId("sidebar-mob-toggle:research").getAttribute("aria-expanded")).toBe("false");
    expect(listing(first.container)).toEqual([
      "sidebar-mob:ops-team",
      "sidebar-agent:dispatcher",
      "sidebar-agent:planner",
      "sidebar-mob:research",
      "sidebar-mob:6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f",
      "sidebar-agent:archivist",
      "sidebar-agent:curator",
    ]);
    first.unmount();
    const second = render(<Sidebar {...props} />);
    expect(second.getByTestId("sidebar-mob-toggle:research").getAttribute("aria-expanded")).toBe("false");
  });

  it("keeps the person's grouping choice, and offers Category and None beside Mob", () => {
    const first = render(<Sidebar {...props} />);
    fireEvent.click(first.getByTestId("sidebar-view-menu"));
    fireEvent.click(first.getByTestId("sidebar-view-group:category"));
    expect(first.queryByTestId("sidebar-mob:research")).toBeNull();
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toMatchObject({ group_by: "category" });
    first.unmount();
    const second = render(<Sidebar {...props} />);
    expect(second.queryByTestId("sidebar-mob:research")).toBeNull();
    fireEvent.click(second.getByTestId("sidebar-view-menu"));
    fireEvent.click(second.getByTestId("sidebar-view-group:mob"));
    expect(second.getByTestId("sidebar-mob:research")).toBeTruthy();
    expect(JSON.parse(localStorage.getItem(VIEW_KEY) || "{}")).toMatchObject({ group_by: "mob" });
  });

  it("lists no mob header and offers no Mob grouping when the console knows one mob", () => {
    const view = render(<Sidebar {...props} mobs={[MOBS[0]!]} agents={AGENTS.slice(0, 2)} />);
    expect(view.queryByTestId("sidebar-mob:ops-team")).toBeNull();
    fireEvent.click(view.getByTestId("sidebar-view-menu"));
    expect(view.queryByTestId("sidebar-view-group:mob")).toBeNull();
  });
});

describe("Roster mob choice", () => {
  it("shows one mob with the mobs nested under it, and says so in the view summary", () => {
    const view = render(<Sidebar {...props} selectedMobId="research" onSelectMob={vi.fn()} />);
    expect(listing(view.container)).toEqual([
      "sidebar-mob:research",
      "sidebar-agent:researcher",
      "sidebar-mob:reading-room",
      "sidebar-agent:reader",
    ]);
    expect(view.getByTestId("sidebar-agent-count").textContent).toBe("2 of 6 agents");
    expect(view.getByTestId("sidebar-view-summary").textContent).toContain("Planner's research team");
  });

  it("chooses a mob from its discreet menu and goes back to all mobs from the view reset", () => {
    const onSelectMob = vi.fn();
    const view = render(<Sidebar {...props} onSelectMob={onSelectMob} />);
    const trigger = view.getByTestId("sidebar-mob-menu:research");
    expect(trigger.getAttribute("aria-label")).toBe("More actions for Planner's research team");
    expect(trigger.tagName).toBe("BUTTON");
    fireEvent.click(trigger);
    fireEvent.click(view.getByTestId("sidebar-mob-only:research"));
    expect(onSelectMob).toHaveBeenLastCalledWith("research");
    view.rerender(<Sidebar {...props} selectedMobId="research" onSelectMob={onSelectMob} />);
    fireEvent.click(view.getByTestId("sidebar-view-reset"));
    expect(onSelectMob).toHaveBeenLastCalledWith(null);
  });

  it("puts no control inside another and no mob id in a header's text", () => {
    const view = render(<Sidebar {...props} onSelectMob={vi.fn()} />);
    expect(view.container.querySelector("button button, button [role='button']")).toBeNull();
    // A readable id reads in words ("reading-room" as "reading room"): the
    // id as written is in no header.
    for (const mob of MOBS.filter((entry) => /[-\d]/.test(entry.mob_id))) {
      const header = view.getByTestId(`sidebar-mob:${mob.mob_id}`);
      expect(within(header).getByTestId(`sidebar-mob-toggle:${mob.mob_id}`).textContent).not.toContain(mob.mob_id);
    }
  });
});
