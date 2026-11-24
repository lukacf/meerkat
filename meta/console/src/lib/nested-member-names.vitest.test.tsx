import { cleanup, render, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { IdentityInspectViewState } from "@console-core";
import { AgentDirectoryProvider, AgentName, agentDisplayName, createAgentDirectory } from "../../../packages/console-components/src/panel-kit";
import { buildGraph } from "../../../packages/console-components/src/topology/data";
import { AgentDetailsPanel } from "../panels/AgentDetailsPanel";
import { Sidebar } from "../panels/Sidebar";
import type { ConsoleAgent, ConsoleMob } from "../types";
import { buildDockTarget, buildInspectTarget } from "./adapters";
import { agentDirectoryEntry } from "./agents";

// A member of a mob other than the console's own has the identity
// `<mob id>/<member name>`. No surface shows that string as a name: the
// visible text is the display name, else the member name; the identity
// stays in tooltips, test ids and copy actions.

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const QUALIFIED = "research-team/reviewer";

const row = (id: string, mobId: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  agent_id: id,
  member_id: id,
  identity: id,
  // A row without a display name: the console fills the label from the identity.
  label: id,
  kind: "mob_agent",
  role: "analyst",
  mob_id: mobId,
  ...extra,
});

const PLANNER = row("planner", "ops-team", { label: "Planner", role: "coordinator" });
const UNNAMED = row(QUALIFIED, "research-team", { wired_to: ["planner"] });
const NAMED = row(QUALIFIED, "research-team", { label: "Senior reviewer", wired_to: ["planner"] });
const MOBS: ConsoleMob[] = [
  { mob_id: "ops-team", member_count: 1, state: "running" },
  { mob_id: "research-team", parent_mob_id: "ops-team", host_identity: "planner", member_count: 1, state: "running" },
];

const sidebarProps = {
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

/** Every text a person can read or hear in `element`: its text and its aria-labels. */
function readableText(element: HTMLElement): string {
  const labels = [...element.querySelectorAll("[aria-label]")].map((node) => node.getAttribute("aria-label") || "");
  return [element.textContent || "", ...labels].join("\n");
}

describe("the roster row of a nested member", () => {
  it("shows the member name when the row has no display name, the identity only in the tooltip", () => {
    const view = render(<Sidebar {...sidebarProps} agents={[PLANNER, UNNAMED]} />);
    const agentRow = view.getByTestId(`sidebar-agent:${QUALIFIED}`);
    expect(agentRow.querySelector(".agent__name")?.textContent).toBe("Reviewer");
    expect(agentRow.getAttribute("title")).toBe(`Identity: ${QUALIFIED}`);
    const list = view.getByTestId("sidebar-agent-list");
    expect(readableText(list).includes(QUALIFIED)).toBe(false);
    expect(readableText(list).includes("Research team reviewer")).toBe(false);
    expect(view.getByTestId(`sidebar-agent-pin:${QUALIFIED}`).getAttribute("aria-label")).toBe("Pin Reviewer");
    const copy = view.getByTestId(`sidebar-agent-copy-identity:${QUALIFIED}`).querySelector("button");
    expect(copy?.getAttribute("aria-label")).toBe("Copy the identity of Reviewer");
  });

  it("shows the display name when the row has one", () => {
    const view = render(<Sidebar {...sidebarProps} agents={[PLANNER, NAMED]} />);
    const agentRow = view.getByTestId(`sidebar-agent:${QUALIFIED}`);
    expect(agentRow.querySelector(".agent__name")?.textContent).toBe("Senior reviewer");
    expect(readableText(view.getByTestId("sidebar-agent-list")).includes(QUALIFIED)).toBe(false);
  });
});

describe("the panes of a nested member", () => {
  it("titles its chat and details panes by name, and keeps the mob out of the subtitle", () => {
    expect(buildDockTarget(UNNAMED)).toMatchObject({ title: "Reviewer", identity: QUALIFIED, memberId: QUALIFIED });
    expect(buildDockTarget(NAMED).title).toBe("Senior reviewer");
    const inspect = buildInspectTarget(UNNAMED);
    expect(inspect).toMatchObject({ title: "Reviewer Details", identity: QUALIFIED, subtitle: "reviewer" });
    expect(buildInspectTarget(NAMED).title).toBe("Senior reviewer Details");
    // A primary member's subtitle is its identity, as before.
    expect(buildInspectTarget(PLANNER).subtitle).toBe("planner");
  });

  it("names it in the details panel's title and menu, the identity in the tooltip and the technical details", () => {
    const directory = createAgentDirectory([PLANNER, UNNAMED].map(agentDirectoryEntry));
    const inspect = {
      identity: QUALIFIED,
      display_name: QUALIFIED,
      role: "analyst",
      state: "active",
      addressability: "addressable",
      labels: {},
      continuity: { generation: 1 },
      topology_peers: ["planner"],
    } as unknown as IdentityInspectViewState;
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel identity={QUALIFIED} inspect={inspect} agent={UNNAMED} agents={[PLANNER, UNNAMED]} stateLabel="active" actions={{}} />
      </AgentDirectoryProvider>,
    );
    const panel = view.getByTestId(`inspect-panel:${QUALIFIED}`);
    const heading = within(panel).getByRole("heading", { level: 2 });
    expect(heading.textContent).toBe("Reviewer");
    expect(heading.querySelector("[title]")?.getAttribute("title")).toBe(QUALIFIED);
    expect(view.getByTestId(`inspect-menu:${QUALIFIED}`).getAttribute("aria-label")).toBe("More actions for Reviewer");
    // Without the row (a member that left), the directory still names it.
    cleanup();
    const gone = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentDetailsPanel identity="research-team/archivist" inspect={{ ...inspect, identity: "research-team/archivist", display_name: "research-team/archivist" } as IdentityInspectViewState} agent={undefined} stateLabel="retired" actions={{}} />
      </AgentDirectoryProvider>,
    );
    expect(within(gone.getByTestId("inspect-panel:research-team/archivist")).getByRole("heading", { level: 2 }).textContent).toBe("Archivist");
  });
});

describe("names the panels resolve from an identity", () => {
  const directory = createAgentDirectory([PLANNER, UNNAMED].map(agentDirectoryEntry));

  it("names a listed nested member by its row, and one no row names by its member name", () => {
    expect(agentDisplayName(QUALIFIED, directory)).toBe("Reviewer");
    expect(agentDisplayName("research-team/archivist", directory)).toBe("Archivist");
    expect(agentDisplayName("identity:research-team/archivist", directory)).toBe("Archivist");
    // Neither a runtime namespace nor a mob the console does not list is dropped.
    expect(agentDisplayName("east/archivist", directory)).toBe("East archivist");
    expect(agentDisplayName("ops-team/archivist", directory)).toBe("Ops team archivist");
    expect(agentDirectoryEntry(NAMED).label).toBe("Senior reviewer");
  });

  it("shows the name and keeps the identity as the tooltip (peer chips, senders, owners)", () => {
    const view = render(
      <AgentDirectoryProvider directory={directory}>
        <AgentName id={QUALIFIED} testId="known" />
        <AgentName id="research-team/archivist" testId="gone" />
      </AgentDirectoryProvider>,
    );
    expect(view.getByTestId("known").textContent).toBe("Reviewer");
    expect(view.getByTestId("known").getAttribute("title")).toBe(QUALIFIED);
    expect(view.getByTestId("gone").textContent).toBe("Archivist");
    expect(view.getByTestId("gone").getAttribute("title")).toBe("research-team/archivist");
  });
});

describe("the topology node of a nested member", () => {
  it("is labelled by name, with or without a display name on the node", () => {
    const unnamed = buildGraph(
      [
        { identity: "planner", label: "Planner", role: "coordinator", wired_to: [QUALIFIED], labels: {}, mob_id: "ops-team" },
        { identity: QUALIFIED, label: QUALIFIED, role: "analyst", wired_to: ["planner"], labels: {}, mob_id: "research-team" },
      ],
      [],
    );
    expect(unnamed.agents.map((agent) => [agent.id, agent.label])).toEqual([["planner", "Planner"], [QUALIFIED, "Reviewer"]]);
    const fromRoster = buildGraph([{ identity: QUALIFIED, role: "analyst", wired_to: [], labels: {} }], [NAMED]);
    expect(fromRoster.agents.map((agent) => [agent.id, agent.label])).toEqual([[QUALIFIED, "Senior reviewer"]]);
    const unnamedRoster = buildGraph([{ identity: QUALIFIED, role: "analyst", wired_to: [], labels: {} }], [UNNAMED]);
    expect(unnamedRoster.agents.map((agent) => agent.label)).toEqual(["Reviewer"]);
  });
});
