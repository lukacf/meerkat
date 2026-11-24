import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { TopologyPanel } from "../../../packages/console-components/src/topology/topology-panel";
import { Sidebar } from "./Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";

// A mob's header keeps its name readable however deep the mob is nested,
// and its Copy mob ID action copies on any origin and says so.

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
  vi.unstubAllGlobals();
  vi.useRealTimers();
  delete (document as { execCommand?: unknown }).execCommand;
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

// Six levels under the console's own mob: level-1 is nested in ops-team,
// level-2 in level-1, and so on.
const LEVELS = [1, 2, 3, 4, 5, 6];
const MOBS: ConsoleMob[] = [
  { mob_id: "ops-team", member_count: 1, state: "running" },
  ...LEVELS.map((level) => ({
    mob_id: `level-${level}`,
    parent_mob_id: level === 1 ? "ops-team" : `level-${level - 1}`,
    member_count: 1,
    state: "running",
  })),
];
const AGENTS: ConsoleAgent[] = [
  agent("planner", "Planner", "ops-team"),
  ...LEVELS.map((level) => agent(`level-${level}/worker`, `Worker ${level}`, `level-${level}`)),
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

const indentOf = (element: HTMLElement | null) => element?.style.getPropertyValue("--sidebar-mob-indent") || "";

describe("a deeply nested mob", () => {
  it("steps its header in once per level up to a cap, and its agents with it", () => {
    const view = render(<Sidebar {...props} />);
    const header = (mobId: string) => view.getByTestId(`sidebar-mob:${mobId}`);
    expect(LEVELS.map((level) => header(`level-${level}`).getAttribute("data-depth"))).toEqual(["1", "2", "3", "4", "5", "6"]);
    expect(indentOf(header("ops-team"))).toBe("0px");
    expect(LEVELS.map((level) => indentOf(header(`level-${level}`)))).toEqual(["8px", "16px", "24px", "24px", "24px", "24px"]);
    // An agent steps in with its mob's header; an agent of the top-level mob does not.
    const rowOf = (memberId: string) => view.getByTestId(`sidebar-agent:${memberId}`).closest<HTMLElement>(".sidebar__virtual-row");
    expect(indentOf(rowOf("planner"))).toBe("");
    expect(indentOf(rowOf("level-2/worker"))).toBe("16px");
    expect(indentOf(rowOf("level-6/worker"))).toBe("24px");
    // The tooltip says where the mob is nested, whatever its indent.
    expect(view.getByTestId("sidebar-mob-toggle:level-6").getAttribute("title")).toContain("In Level 5 team");
  });

  it("is stepped in up to the same cap in the topology's mob choice", () => {
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={[{ identity: "planner", label: "Planner", role: "coordinator", wired_to: [], labels: {} }]}
        mobChoice={{
          mobs: [{ mobId: "ops-team", name: "Ops team", depth: 0 }, ...LEVELS.map((level) => ({ mobId: `level-${level}`, name: `Level ${level} team`, depth: level }))],
          selected: null,
          onSelect: vi.fn(),
        }}
      />,
    );
    fireEvent.click(screen.getByTestId("topology-menu"));
    const menu = screen.getByRole("menu");
    const indent = (mobId: string) =>
      within(menu).getByTestId(`topology-mob:${mobId}`).querySelector<HTMLElement>(".topo__mob-option")?.style.paddingInlineStart;
    expect(LEVELS.map((level) => indent(`level-${level}`))).toEqual(["12px", "24px", "36px", "48px", "48px", "48px"]);
  });
});

describe("Copy mob ID", () => {
  const copyFrom = async (view: ReturnType<typeof render>, mobId: string) => {
    fireEvent.click(view.getByTestId(`sidebar-mob-menu:${mobId}`));
    await act(async () => {
      fireEvent.click(view.getByTestId(`sidebar-mob-copy:${mobId}`));
    });
  };

  it("copies through the page when the origin has no clipboard API, and says it copied", async () => {
    // A plain-http origin: no navigator.clipboard. The console's helper
    // copies through a selection instead.
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    let copiedText = "";
    (document as { execCommand?: unknown }).execCommand = vi.fn(() => {
      copiedText = (document.activeElement as HTMLTextAreaElement | null)?.value ?? "";
      return true;
    });
    const view = render(<Sidebar {...props} />);
    await copyFrom(view, "level-1");
    await waitFor(() => expect(view.queryByTestId("sidebar-mob-copied:level-1") !== null).toBe(true));
    expect(copiedText).toBe("level-1");
    const status = view.getByTestId("sidebar-mob-copied:level-1");
    expect([status.textContent, status.getAttribute("role")]).toEqual(["Mob ID copied", "status"]);
    expect(view.queryByTestId("sidebar-mob-copied:level-2") === null).toBe(true);
  });

  it("copies through the clipboard API where the origin has one", async () => {
    const writeText = vi.fn(async () => undefined);
    vi.stubGlobal("navigator", { ...navigator, clipboard: { writeText } });
    const view = render(<Sidebar {...props} />);
    await copyFrom(view, "level-2");
    await waitFor(() => expect(view.queryByTestId("sidebar-mob-copied:level-2") !== null).toBe(true));
    expect(writeText.mock.calls).toEqual([["level-2"]]);
    expect(view.getByTestId("sidebar-mob-copied:level-2").textContent).toBe("Mob ID copied");
  });

  it("says so when nothing could be copied", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    const view = render(<Sidebar {...props} />);
    await copyFrom(view, "level-1");
    await waitFor(() => expect(view.queryByTestId("sidebar-mob-copied:level-1") !== null).toBe(true));
    const status = view.getByTestId("sidebar-mob-copied:level-1");
    expect([status.textContent, status.getAttribute("data-copied")]).toEqual(["Could not copy", "false"]);
  });
});
