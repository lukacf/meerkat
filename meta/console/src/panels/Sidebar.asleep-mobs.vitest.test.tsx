import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { consoleMobTree, mobState, mobStateTone, mobStateWords } from "../../../packages/console-core/src/mobs";
import type { ConsoleAgent, ConsoleMob } from "../types";
import { Sidebar } from "./Sidebar";

// A mob the host created that is not loaded is listed with no member and no
// count: asleep, or unavailable when its store does not load. The roster
// says so in words, shows no number for it, and opening it is a person's
// act: the roster reports it and asks nothing of the gateway itself.

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const PLANNER: ConsoleAgent = {
  agent_id: "planner",
  member_id: "planner",
  identity: "planner",
  label: "Planner",
  kind: "mob_agent",
  role: "coordinator",
  mob_id: "ops-team",
};
const MOBS = [
  { mob_id: "ops-team", member_count: 1, state: "running", provenance: "host", default: true },
  { mob_id: "archive", state: "dormant", provenance: "host" },
  { mob_id: "billing", state: "unavailable", provenance: "host" },
] as ConsoleMob[];

const props = {
  mobs: MOBS,
  agents: [PLANNER],
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

/** Every text a person can read or hear in `element`: its text, its aria-labels and its tooltips. */
function readableText(element: HTMLElement): string {
  const attributes = [...element.querySelectorAll("[aria-label], [title]")]
    .flatMap((node) => [node.getAttribute("aria-label") || "", node.getAttribute("title") || ""]);
  return [element.textContent || "", ...attributes].join("\n");
}

describe("a listed mob's state", () => {
  it("reads asleep and unavailable in words, and any other new word as unknown", () => {
    expect([mobState("dormant"), mobStateWords("dormant"), mobStateTone("dormant")]).toEqual(["dormant", "Asleep", "neutral"]);
    expect([mobState("unavailable"), mobStateWords("unavailable"), mobStateTone("unavailable")]).toEqual(["unavailable", "Unavailable", "crit"]);
    expect([mobState("hibernating"), mobStateWords("hibernating")]).toEqual(["unknown", "Status unknown"]);
  });

  it("places a mob that is not loaded as a root with no members counted", () => {
    const tree = consoleMobTree(MOBS, "ops-team");
    expect(tree.map((mob) => [mob.mobId, mob.state, mob.loaded, mob.memberCount])).toEqual([
      ["ops-team", "running", true, 1],
      ["archive", "dormant", false, 0],
      ["billing", "unavailable", false, 0],
    ]);
  });
});

describe("what the roster keeps for a mob the console forgot", () => {
  // Pins and the manual order are presentation, kept per person. They are
  // not removed with a forgotten mob (one created again under its id, or
  // gone from the list). They only order and mark rows that are listed:
  // they never make a member appear, and so never make one addressable.
  const FORGOTTEN = "support/reviewer";
  const ORDER_KEY = "meta-console-sidebar-agent-order:principal-a";

  it("shows no row for a pinned or ordered member of a mob that is not listed", () => {
    localStorage.setItem(ORDER_KEY, JSON.stringify([FORGOTTEN, "planner"]));
    const onSelect = vi.fn();
    const view = render(<Sidebar {...props} onSelect={onSelect} pinnedAgentIds={new Set([FORGOTTEN, "planner"])} />);
    expect(view.queryByTestId(`sidebar-agent:${FORGOTTEN}`)).toBeNull();
    expect(view.queryByTestId(`sidebar-agent-pin:${FORGOTTEN}`)).toBeNull();
    expect(view.queryByTestId("sidebar-mob:support")).toBeNull();
    const list = view.getByTestId("sidebar-agent-list");
    expect(readableText(list).toLowerCase().includes("reviewer")).toBe(false);
    // Every row the roster offers is a listed member's.
    const offered = [...list.querySelectorAll("[data-testid^='sidebar-agent:']")].map((node) => node.getAttribute("data-testid"));
    expect(new Set(offered)).toEqual(new Set(["sidebar-agent:planner"]));
    for (const row of list.querySelectorAll("[data-testid^='sidebar-agent:']")) fireEvent.click(row);
    expect(onSelect.mock.calls.every(([agent]) => (agent as ConsoleAgent).member_id === "planner")).toBe(true);
  });

  it("gives a member of a mob created again under that id its row, pinned as the person left it", () => {
    const again = { ...PLANNER, agent_id: FORGOTTEN, member_id: FORGOTTEN, identity: FORGOTTEN, label: "Reviewer", mob_id: "support" };
    const mobs = [MOBS[0]!, { mob_id: "support", member_count: 1, state: "running", provenance: "host", incarnation: "two" }] as ConsoleMob[];
    const view = render(<Sidebar {...props} mobs={mobs} agents={[PLANNER, again]} pinnedAgentIds={new Set([FORGOTTEN])} />);
    // The pin is a mark on a listed row: the row is there because the roster lists it.
    expect(view.getAllByTestId(`sidebar-agent:${FORGOTTEN}`).length).toBeGreaterThan(0);
  });
});

describe("the roster's header of a mob that is not loaded", () => {
  it("says asleep or unavailable, and shows no count", () => {
    const view = render(<Sidebar {...props} />);
    expect(view.getByTestId("sidebar-mob-state:archive").textContent).toBe("Asleep");
    expect(view.getByTestId("sidebar-mob-state:billing").textContent).toBe("Unavailable");
    expect(view.queryByTestId("sidebar-mob-count:archive")).toBeNull();
    expect(view.queryByTestId("sidebar-mob-count:billing")).toBeNull();
    expect(view.getByTestId("sidebar-mob-count:ops-team").textContent).toBe("1");
    expect(view.getByTestId("sidebar-mob-toggle:archive").getAttribute("aria-label")).toBe("Archive team, Asleep");
    const archive = readableText(view.getByTestId("sidebar-mob:archive"));
    expect(archive.includes("0 agents")).toBe(false);
    expect(archive.includes("dormant")).toBe(false);
  });

  it("reports the person's opening of an asleep mob, by its header and by choosing it", () => {
    const onOpenMob = vi.fn();
    const onSelectMob = vi.fn();
    const view = render(<Sidebar {...props} onOpenMob={onOpenMob} onSelectMob={onSelectMob} />);
    expect(onOpenMob).not.toHaveBeenCalled();
    fireEvent.click(view.getByTestId("sidebar-mob-toggle:archive"));
    expect(onOpenMob.mock.calls).toEqual([["archive"]]);
    // A running mob's header only folds and unfolds.
    fireEvent.click(view.getByTestId("sidebar-mob-toggle:ops-team"));
    expect(onOpenMob.mock.calls).toEqual([["archive"]]);
  });

  it("offers no opening of a mob that is unavailable: its store does not load, and a click asks for nothing", () => {
    const onOpenMob = vi.fn();
    const onSelectMob = vi.fn();
    const view = render(<Sidebar {...props} onOpenMob={onOpenMob} onSelectMob={onSelectMob} />);
    fireEvent.click(view.getByTestId("sidebar-mob-toggle:billing"));
    expect(onOpenMob).not.toHaveBeenCalled();
    expect(view.getByTestId("sidebar-mob-state:billing").textContent).toBe("Unavailable");
  });

  it("says waking while the person's opening is under way", () => {
    const view = render(<Sidebar {...props} onOpenMob={vi.fn()} wakingMobIds={new Set(["archive"])} />);
    expect(view.getByTestId("sidebar-mob-state:archive").textContent).toBe("Waking");
    expect(view.getByTestId("sidebar-mob-toggle:archive").getAttribute("aria-label")).toBe("Archive team, Waking");
  });
});
