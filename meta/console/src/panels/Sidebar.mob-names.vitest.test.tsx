import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { TopologyPanel } from "../../../packages/console-components/src/topology/topology-panel";
import { Sidebar } from "./Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";

// A mob's header, menu, summary and topology entry name the mob for people:
// by the member that created it, by its id's words, by an ordinal. Never by
// its id or a piece of it, which only the tooltip and the copy action carry.

// The roster lists only the rows its measured height shows: give it room for
// every row of these rosters.
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

const GENERATED_A = "6f1c2d3e-4a5b-4c6d-8e7f-9a0b1c2d3e4f";
const GENERATED_B = "9b8a7c6d-5e4f-4a3b-8c2d-1e0f9a8b7c6d";
const GENERATED_HOSTED = "0a1b2c3d-4e5f-4a6b-8c7d-9e0f1a2b3c4d";
const IMPLICIT = "implicit-0193b2a4-77c1-7d2e-9f10-2b3c4d5e6f70";

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
  { mob_id: "ops-team", member_count: 1, state: "running" },
  { mob_id: "research-team", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1, state: "running" },
  { mob_id: IMPLICIT, parent_mob_id: "research-team", host_identity: "research-team/researcher", member_count: 1, state: "running" },
  { mob_id: GENERATED_HOSTED, parent_mob_id: "ops-team", host_identity: "planner", member_count: 1, state: "stopped" },
  // No creator named: the caller may not see the member that created them.
  { mob_id: GENERATED_A, member_count: 1, state: "running" },
  { mob_id: GENERATED_B, member_count: 1, state: "paused-by-a-later-release" },
  // Two ids that read the same in words.
  { mob_id: "library", member_count: 1, state: "running" },
  { mob_id: "library_team", member_count: 1, state: "running" },
];
const AGENTS: ConsoleAgent[] = [
  agent("planner", "Planner", "ops-team", { role: "coordinator" }),
  agent("research-team/researcher", "Researcher", "research-team"),
  agent(`${IMPLICIT}/helper-1`, "Helper", IMPLICIT, { role: "helper" }),
  agent(`${GENERATED_HOSTED}/scout`, "Scout", GENERATED_HOSTED),
  agent(`${GENERATED_A}/archivist`, "Archivist", GENERATED_A),
  agent(`${GENERATED_B}/curator`, "Curator", GENERATED_B),
  agent("library/reader", "Reader", "library"),
  agent("library_team/binder", "Binder", "library_team"),
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
  onSelectMob: vi.fn(),
  storageNamespace: "configuration-a",
  preferenceScope: "principal-a",
};

const headerText = (view: ReturnType<typeof render>, mobId: string) =>
  view.getByTestId(`sidebar-mob-toggle:${mobId}`).querySelector(".sidebar__mob-label")?.textContent;

/** Whether text shows six characters in a row of a generated id. */
function showsIdPiece(text: string, id: string): boolean {
  const compactText = text.toLowerCase().replace(/[^a-z0-9]+/g, "");
  const compactId = id.replace(/^implicit-/, "").toLowerCase().replace(/[^a-z0-9]+/g, "");
  for (let start = 0; start + 6 <= compactId.length; start += 1) {
    if (compactText.includes(compactId.slice(start, start + 6))) return true;
  }
  return false;
}

describe("the roster's mob headers", () => {
  it("name a mob by the member that created it, by its id's words, or by an ordinal", () => {
    const view = render(<Sidebar {...props} />);
    expect(headerText(view, "ops-team")).toBe("Ops team");
    expect(headerText(view, "research-team")).toBe("Planner's research team");
    expect(headerText(view, IMPLICIT)).toBe("Researcher's helpers");
    expect(headerText(view, GENERATED_HOSTED)).toBe("Planner's team");
    expect(headerText(view, GENERATED_A)).toBe("Team 1");
    expect(headerText(view, GENERATED_B)).toBe("Team 2");
  });

  it("tell two mobs of one name apart in visible secondary text, not by an id", () => {
    const view = render(<Sidebar {...props} />);
    expect(headerText(view, "library")).toBe("Library team");
    expect(headerText(view, "library_team")).toBe("Library team");
    expect(view.getByTestId("sidebar-mob-detail:library").textContent).toBe("1 of 2");
    expect(view.getByTestId("sidebar-mob-detail:library_team").textContent).toBe("2 of 2");
    expect(view.queryByTestId("sidebar-mob-detail:research-team") === null).toBe(true);
    expect(view.getByTestId("sidebar-mob-menu:library_team").getAttribute("aria-label")).toBe("More actions for Library team (2 of 2)");
  });

  it("show no id and no piece of an id in what a person reads or hears, only in the tooltip", () => {
    const view = render(<Sidebar {...props} />);
    for (const id of [GENERATED_A, GENERATED_B, GENERATED_HOSTED, IMPLICIT]) {
      const header = view.getByTestId(`sidebar-mob:${id}`);
      const toggle = view.getByTestId(`sidebar-mob-toggle:${id}`);
      const read = [
        header.textContent || "",
        toggle.getAttribute("aria-label") || "",
        view.getByTestId(`sidebar-mob-menu:${id}`).getAttribute("aria-label") || "",
      ].join("\n");
      expect(showsIdPiece(read, id), id).toBe(false);
      expect(toggle.getAttribute("title")?.includes(`Mob ID ${id}`)).toBe(true);
    }
    expect(view.getByTestId("sidebar-mob-toggle:research-team").getAttribute("title")).toContain("Created by Planner");
    expect(view.getByTestId(`sidebar-mob-toggle:${IMPLICIT}`).getAttribute("title")).toContain("In Planner's research team");
    // No creator is named for a mob whose entry carries none.
    expect(view.getByTestId(`sidebar-mob-toggle:${GENERATED_A}`).getAttribute("title")?.includes("Created by")).toBe(false);
  });

  it("show no id of a mob the host created beside the launch mob: loaded, asleep, unavailable or waking", () => {
    // Three roots with generated ids: one loaded, with a member that has no
    // display name; one asleep, one unavailable, one asleep and being opened.
    const LOADED = "3c4d5e6f-7a8b-4c9d-8e0f-1a2b3c4d5e6f";
    const ASLEEP = "5e6f7a8b-9c0d-4e1f-8a2b-3c4d5e6f7a8b";
    const UNAVAILABLE = "7a8b9c0d-1e2f-4a3b-8c4d-5e6f7a8b9c0d";
    const WAKING = "9c0d1e2f-3a4b-4c5d-8e6f-7a8b9c0d1e2f";
    const hostedRoots = [
      { mob_id: "ops-team", member_count: 1, state: "running", provenance: "host", default: true },
      { mob_id: LOADED, member_count: 1, state: "running", provenance: "host", incarnation: "one" },
      { mob_id: ASLEEP, state: "dormant", provenance: "host", incarnation: "two" },
      { mob_id: UNAVAILABLE, state: "unavailable", provenance: "host", incarnation: "three" },
      { mob_id: WAKING, state: "dormant", provenance: "host", incarnation: "four" },
    ] as ConsoleMob[];
    const member = `${LOADED}/reviewer`;
    const view = render(
      <Sidebar
        {...props}
        mobs={hostedRoots}
        agents={[AGENTS[0]!, agent(member, member, LOADED)]}
        {...{ onOpenMob: vi.fn(), wakingMobIds: new Set([WAKING]) }}
      />,
    );
    for (const id of [LOADED, ASLEEP, UNAVAILABLE, WAKING]) {
      const header = view.getByTestId(`sidebar-mob:${id}`);
      const toggle = view.getByTestId(`sidebar-mob-toggle:${id}`);
      const read = [
        header.textContent || "",
        toggle.getAttribute("aria-label") || "",
        view.getByTestId(`sidebar-mob-menu:${id}`).getAttribute("aria-label") || "",
      ].join("\n");
      expect(showsIdPiece(read, id), id).toBe(false);
      // The id stays where it always was: the tooltip and the copy action.
      expect(toggle.getAttribute("title")?.includes(`Mob ID ${id}`)).toBe(true);
    }
    // Several roots whose ids say nothing are told apart by an ordinal.
    expect(new Set([LOADED, ASLEEP, UNAVAILABLE, WAKING].map((id) => headerText(view, id))).size).toBe(4);
    // The member of a hosted root reads by its name, as a nested member does.
    const row = view.getByTestId(`sidebar-agent:${member}`);
    expect(row.querySelector(".agent__name")?.textContent).toBe("Reviewer");
    expect(showsIdPiece(row.textContent || "", LOADED)).toBe(false);
    expect(row.getAttribute("title")).toBe(`Identity: ${member}`);
  });

  it("say the state of a mob that is not running in words beside its name", () => {
    const view = render(<Sidebar {...props} />);
    expect(view.getByTestId(`sidebar-mob-state:${GENERATED_HOSTED}`).textContent).toBe("Stopped");
    expect(view.getByTestId(`sidebar-mob-state:${GENERATED_B}`).textContent).toBe("Status unknown");
    expect(view.queryByTestId("sidebar-mob-state:ops-team") === null).toBe(true);
    expect(view.getByTestId(`sidebar-mob:${GENERATED_B}`).getAttribute("data-state")).toBe("unknown");
  });

  it("name the chosen mob in the view summary by the same name", () => {
    const view = render(<Sidebar {...props} selectedMobId={GENERATED_HOSTED} />);
    expect(view.getByTestId("sidebar-view-summary").textContent).toContain("Planner's team");
    view.rerender(<Sidebar {...props} selectedMobId="library_team" />);
    expect(view.getByTestId("sidebar-view-summary").textContent).toContain("Library team (2 of 2)");
  });

  it("keep a row whose mob the console does not list under the console's own mob, with a mob chosen too", () => {
    const stray = agent("stray", "Stray", "not-listed");
    const mobs = MOBS.slice(0, 2);
    const agents = [AGENTS[0]!, AGENTS[1]!, stray];
    const listing = (container: HTMLElement) =>
      [...container.querySelectorAll('[data-testid^="sidebar-mob:"], [data-testid^="sidebar-agent:"]')]
        .map((node) => node.getAttribute("data-testid") || "");
    const all = render(<Sidebar {...props} mobs={mobs} agents={agents} />);
    expect(listing(all.container)).toEqual([
      "sidebar-mob:ops-team",
      "sidebar-agent:planner",
      "sidebar-agent:stray",
      "sidebar-mob:research-team",
      "sidebar-agent:research-team/researcher",
    ]);
    all.rerender(<Sidebar {...props} mobs={mobs} agents={agents} selectedMobId="ops-team" />);
    expect(listing(all.container)).toEqual([
      "sidebar-mob:ops-team",
      "sidebar-agent:planner",
      "sidebar-agent:stray",
      "sidebar-mob:research-team",
      "sidebar-agent:research-team/researcher",
    ]);
    all.rerender(<Sidebar {...props} mobs={mobs} agents={agents} selectedMobId="research-team" />);
    expect(listing(all.container)).toEqual(["sidebar-mob:research-team", "sidebar-agent:research-team/researcher"]);
  });
});

describe("the topology's mob choice", () => {
  it("lists each mob by its name and what tells it apart, never by an id", () => {
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={[{ identity: "planner", label: "Planner", role: "coordinator", wired_to: [], labels: {} }]}
        mobChoice={{
          mobs: [
            { mobId: "ops-team", name: "Ops team", detail: null, depth: 0 },
            { mobId: "library", name: "Library team", detail: "1 of 2", depth: 0 },
            { mobId: "library_team", name: "Library team", detail: "2 of 2", depth: 0 },
            { mobId: GENERATED_A, name: "Team 1", detail: null, depth: 0 },
          ],
          selected: "library_team",
          onSelect: vi.fn(),
        }}
      />,
    );
    expect(screen.getByTestId("topology-mob-choice").textContent).toBe("Library team (2 of 2)");
    fireEvent.click(screen.getByTestId("topology-menu"));
    const menu = screen.getByRole("menu");
    expect(within(menu).getByTestId("topology-mob:library").textContent).toBe("Library team 1 of 2");
    expect(within(menu).getByTestId("topology-mob:library_team").textContent).toBe("Library team 2 of 2");
    expect(within(menu).getByTestId(`topology-mob:${GENERATED_A}`).textContent).toBe("Team 1");
    expect(showsIdPiece(menu.textContent || "", GENERATED_A)).toBe(false);
  });
});
