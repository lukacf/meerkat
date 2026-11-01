import React from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { topologyEdgeKey, type TopologyManagementState } from "@console-core";

import { TopologyPanel } from "./topology-panel";
import type { ConsoleAgent, ConsoleFrame, ConsoleTopologyLink, ConsoleTopologyNode } from "./types";

const NODES: ConsoleTopologyNode[] = [
  {
    identity: "commander",
    label: "Commander",
    role: "coordination",
    group: "Command",
    wired_to: ["triage"],
    presentation: { caption: "Incident lead", section: "Command" },
  },
  {
    identity: "triage",
    label: "Triage",
    role: "analysis",
    group: "Response",
    wired_to: ["commander"],
    presentation: { caption: "Evidence triage", section: "Response" },
  },
  {
    identity: "responder",
    label: "Responder",
    role: "execution",
    group: "Response",
    wired_to: [],
    presentation: { caption: "Remediation", section: "Response" },
  },
];

const management: TopologyManagementState = {
  revision: "rev-12",
  policy: {
    mode: "editable",
    capabilities: {
      connect: { state: "allowed" },
      disconnect: { state: "allowed" },
      reconnect: { state: "allowed" },
    },
  },
  affordances: [
    {
      edge: { from: "commander", to: "triage" },
      state: "connected",
      actions: { disconnect: { state: "allowed" } },
    },
    {
      edge: { from: "commander", to: "responder" },
      state: "disconnected",
      actions: { connect: { state: "allowed" } },
    },
  ],
};

const localStorageValues = new Map<string, string>();
const localStorageStub = {
  clear: () => localStorageValues.clear(),
  getItem: (key: string) => localStorageValues.get(key) ?? null,
  key: (index: number) => Array.from(localStorageValues.keys())[index] ?? null,
  get length() { return localStorageValues.size; },
  removeItem: (key: string) => { localStorageValues.delete(key); },
  setItem: (key: string, value: string) => { localStorageValues.set(key, value); },
};

/** Opens the panel's overflow menu ("More actions for Topology"). */
function openMenu() {
  fireEvent.click(screen.getByTestId("topology-menu"));
  return screen.getByRole("menu");
}

/** Selects a map node the way a pointer does (press and release in place). */
function pressNode(id: string) {
  const node = screen.getByTestId(`topology-node:${id}`);
  fireEvent.pointerDown(node, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
  fireEvent.pointerUp(node, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
}

/** Nodes the way the topology query adapter shapes them (stock endpoint keys). */
const key = (identity: string) => `mk1|ops|${identity}`;
function queryNode(identity: string, extra: Partial<ConsoleTopologyNode> = {}): ConsoleTopologyNode {
  return { identity: key(identity), ref: { id: key(identity), authority: "ops", identity }, role: "specialist", labels: {}, wired_to: [], ...extra };
}
const link = (a: string, b: string, flags: Partial<ConsoleTopologyLink>): ConsoleTopologyLink => ({ from: key(a), to: key(b), actual: false, ...flags });
const agent = (identity: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  identity,
  agent_id: identity,
  member_id: identity,
  label: identity[0]!.toUpperCase() + identity.slice(1),
  kind: "mob_agent",
  state: "active",
  ...extra,
});

describe("TopologyPanel", () => {
  beforeEach(() => {
    Object.defineProperty(window, "localStorage", {
      configurable: true,
      value: localStorageStub,
    });
    localStorageStub.clear();
  });

  test("keeps role nodes passive instead of treating topology clicks as navigation", () => {
    window.localStorage.setItem("meta-console-topology-view", "roles");

    render(<TopologyPanel activity={[]} agents={[]} nodes={NODES} />);

    const node = screen.getByTestId("topology-node:commander");
    expect(node.tagName).toBe("DIV");
    fireEvent.click(node);
    expect(screen.queryByTestId("topology-selection")).toBeNull();
  });

  test("lets a host select an agent from the role view, and marks the pressed view", () => {
    window.localStorage.setItem("meta-console-topology-view", "roles");
    const onSelectAgent = vi.fn();

    render(<TopologyPanel activity={[]} agents={[]} nodes={NODES} onSelectAgent={onSelectAgent} />);

    const node = screen.getByTestId("topology-node:commander");
    expect(node.tagName).toBe("BUTTON");
    fireEvent.click(node);
    expect(onSelectAgent.mock.calls[0]?.[0]).toBe("commander");
    expect(screen.getByTestId("topology-view:roles").getAttribute("aria-pressed")).toBe("true");
    expect(screen.getByTestId("topology-view:graph").getAttribute("aria-pressed")).toBe("false");
    expect(within(screen.getByRole("group", { name: "View" })).getByTestId("topology-view:list")).toBeInTheDocument();
  });

  test("names the roles view's root after the team and reads roles as words", () => {
    const nodes: ConsoleTopologyNode[] = [
      { identity: "router:main", role: "memory-steward", state: "active", wired_to: [], labels: {} },
      { identity: "identity:queue-agent", role: "memory-steward", state: "active", wired_to: [], labels: {} },
    ];
    const { container, rerender } = render(<TopologyPanel activity={[]} agents={[]} nodes={nodes} view="roles" onSelectAgent={vi.fn()} />);
    const labels = () => [...container.querySelectorAll(".topo-roletree__label")].map((label) => label.textContent);
    expect(labels()).toEqual(["Team", "Memory steward"]);
    expect(container.querySelector(".topo-roletree__role .topo-roletree__label")?.getAttribute("title")).toBe("memory-steward");
    expect(screen.getByTestId("topology-node:router:main").textContent).toBe("Router main");
    expect(screen.getByTestId("topology-node:identity:queue-agent").textContent).toBe("Queue agent");
    expect(container.querySelector(".topo-roletree__mob .topo-roletree__label")?.hasAttribute("title")).toBe(false);
    // One role, two agents: the counts read as words.
    expect(container.querySelector(".topo-roletree__mob .topo-roletree__count")?.textContent).toBe("2 agents · 1 role");
    rerender(<TopologyPanel activity={[]} agents={[]} nodes={nodes} view="roles" teamName="Operations team" teamKey="ops" />);
    expect(labels()[0]).toBe("Operations team");
    expect(container.querySelector(".topo-roletree__mob .topo-roletree__label")?.getAttribute("title")).toBe("ops");
  });

  test("does not treat a mutation callback as authority", () => {
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        view="roles"
        onRequestMutation={vi.fn()}
      />,
    );

    within(openMenu()).getByText("Show helpers");
    expect(screen.queryByTestId("topology-view:connections")).toBeNull();
    expect(screen.getByTestId("topology-panel")).toHaveAttribute("data-management-mode", "unavailable");
  });

  test("keeps management hidden when the server disables the feature", () => {
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        view="roles"
        management={{
          ...management,
          policy: { ...management.policy, mode: "disabled", reason: "Disabled by deployment policy" },
        }}
        onRequestMutation={vi.fn()}
      />,
    );

    openMenu();
    expect(screen.queryByTestId("topology-view:connections")).toBeNull();
    expect(screen.getByTestId("topology-panel")).toHaveAttribute("data-management-mode", "disabled");
  });

  test("exposes a controlled search-first connection view from the overflow menu", () => {
    const onViewChange = vi.fn();
    const { rerender } = render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={management}
        view="roles"
        onViewChange={onViewChange}
      />,
    );

    openMenu();
    fireEvent.click(screen.getByTestId("topology-view:connections"));
    expect(onViewChange).toHaveBeenCalledWith("connections");

    rerender(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={management}
        view="connections"
        connectionSourceId="commander"
        onViewChange={onViewChange}
      />,
    );
    expect(screen.getByTestId("connection-picker")).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: "Search endpoints" })).toBeInTheDocument();
    expect(screen.queryByTestId("connection-picker-bulk-actions")).toBeNull();
  });

  test("forwards direct automatic resolution state to the connection roster", () => {
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={{ ...management, affordances: management.affordances.slice(0, 1) }}
        view="connections"
        connectionSourceId="commander"
        interactionMode="direct"
        resolvingPairKeys={new Set([topologyEdgeKey({ from: "commander", to: "responder" })])}
      />,
    );

    expect(screen.queryByText("Not inspected")).toBeNull();
    expect(screen.queryByRole("button", { name: /Check .* connection availability/u })).toBeNull();
    expect(screen.getByText("Loading…")).toBeInTheDocument();
  });

  test("renders read-only pair reasons but never emits a mutation", () => {
    const onRequestMutation = vi.fn();
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={{
          ...management,
          policy: { ...management.policy, mode: "read_only", reason: "Observer role" },
        }}
        view="connections"
        connectionSourceId="commander"
        onRequestMutation={onRequestMutation}
      />,
    );

    const button = screen.getByRole("button", { name: "Disconnect Triage from Commander" });
    expect(button).toBeDisabled();
    expect(screen.getAllByText("Observer role").length).toBeGreaterThan(0);
    fireEvent.click(button);
    expect(onRequestMutation).not.toHaveBeenCalled();
  });

  test("keeps the graph passive when management is read-only", () => {
    render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={{
          ...management,
          policy: { ...management.policy, mode: "read_only", reason: "Observer role" },
        }}
        view="graph"
        onRequestMutation={vi.fn()}
      />,
    );

    expect(screen.getByTestId("topology-dense-map")).toHaveAttribute(
      "data-topology-editable",
      "false",
    );
    expect(screen.getByTestId("topology-dense-map")).toHaveAttribute("data-drag-kind", "none");
    expect(screen.queryByText(/drag an agent onto an authorized peer/i)).toBeNull();
  });

  test("enables graph mutations only when a projected gesture is actionable", () => {
    const { rerender } = render(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={management}
        view="graph"
        onRequestMutation={vi.fn()}
      />,
    );

    expect(screen.getByTestId("topology-dense-map")).toHaveAttribute(
      "data-topology-editable",
      "true",
    );

    rerender(
      <TopologyPanel
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={{
          ...management,
          affordances: management.affordances.map((affordance) => ({
            ...affordance,
            actions: Object.fromEntries(
              Object.entries(affordance.actions).map(([action, capability]) => [
                action,
                { ...capability, state: "denied" as const },
              ]),
            ),
          })),
        }}
        view="graph"
        onRequestMutation={vi.fn()}
      />,
    );

    expect(screen.getByTestId("topology-dense-map")).toHaveAttribute(
      "data-topology-editable",
      "false",
    );
  });

  test("draws every relation on the query path, keyed by identity, and keeps the live signal", () => {
    const now = Date.now();
    const frames: ConsoleFrame[] = [
      { id: "run", event: "run_started", identity: "dispatch", timestampMs: now - 2_000, data: {} },
      {
        id: "notice",
        event: "system_notice",
        identity: "billing",
        timestampMs: now - 1_000,
        data: { blocks: [{ type: "comms", kind: "message", direction: "incoming", peer: { display_name: "ops/coordinator/dispatch" }, delivery_id: "d1" }] },
      },
    ];
    render(
      <TopologyPanel
        view="graph"
        activity={frames}
        agents={[agent("billing", { group: "Domain" }), agent("dispatch", { group: "Coordinators" })]}
        nodes={[
          queryNode("billing"),
          queryNode("dispatch", { role: "coordinator" }),
          queryNode("returns"),
          queryNode("refund-review", { labels: { side_chat_of: "billing" } }),
        ]}
        links={[
          link("billing", "dispatch", { actual: true, declared: true, desired: true }),
          link("billing", "returns", { declared: true, desired: true }),
        ]}
        routes={[{ key: "billing-events", channel: "email", recipient: "billing" }]}
        routesAccess="loaded"
      />,
    );
    // Stable node ids: the identity, never the endpoint key.
    expect(screen.getByTestId("topology-node:billing")).toBeInTheDocument();
    expect(screen.queryByTestId(`topology-node:${key("billing")}`)).toBeNull();
    expect(screen.getByTestId("topology-edge:billing:dispatch:wired")).toHaveAttribute("data-state", "normal");
    expect(screen.getByTestId("topology-edge:billing:returns:wired")).toHaveAttribute("data-state", "declared");
    expect(screen.getByTestId("topology-edge:billing:refund-review:lineage")).toHaveAttribute("data-relation", "lineage");
    expect(screen.getByTestId("topology-edge:channel:email:billing:route")).toHaveAttribute("data-relation", "route");
    expect(screen.getByTestId("topology-edge:dispatch:billing:traffic")).toHaveAttribute("data-relation", "traffic");
    // The open run marks the node live although the node came from the query.
    expect(screen.getByTestId("topology-node:dispatch")).toHaveAttribute("data-live", "true");
    expect(screen.getByTestId("topology-panel")).toHaveAttribute("data-busy-count", "1");
    expect(screen.getByTestId("topology-glance:agents")).toHaveTextContent("4");
    expect(screen.getByTestId("topology-glance:connections")).toHaveTextContent("1 wired");
    expect(screen.queryByTestId("topology-empty-relations")).toBeNull();
  });

  test("explains an empty map instead of drawing nothing", () => {
    render(<TopologyPanel view="graph" activity={[]} agents={[]} nodes={[queryNode("a"), queryNode("b")]} links={[]} />);
    const notice = screen.getByTestId("topology-empty-relations");
    expect(notice).toHaveTextContent("No connections yet");
    expect(notice).toHaveTextContent("not wired to each other, no route feeds them");
    expect(screen.getByTestId("topology-glance:connections")).toHaveTextContent("none");
    expect(screen.queryByTestId("topology-legend")).toBeNull();
  });

  test("a failed live read keeps the map and says it shows the last roster", () => {
    render(<TopologyPanel view="graph" activity={[]} agents={[]} nodes={NODES} error="fetch failed: ECONNREFUSED" updatedAtMs={Date.now() - 60_000} />);
    expect(screen.getByTestId("topology-error")).toHaveTextContent("Live topology unavailable, showing the last roster");
    expect(screen.getByTestId("topology-node:commander")).toBeInTheDocument();
  });

  test("opens the drawer on an agent and disconnects a peer from it", () => {
    const onRequestMutation = vi.fn();
    render(
      <TopologyPanel
        view="graph"
        activity={[]}
        agents={[]}
        nodes={NODES}
        management={management}
        onRequestMutation={onRequestMutation}
      />,
    );
    pressNode("commander");
    const drawer = screen.getByTestId("topology-drawer");
    expect(drawer).toHaveTextContent("Commander");
    const row = within(drawer).getByTestId("topology-drawer-connection:triage");
    expect(row).toHaveTextContent("wired");
    fireEvent.click(within(row).getByRole("button", { name: "Disconnect Triage from Commander" }));
    expect(onRequestMutation).toHaveBeenCalledWith(expect.objectContaining({
      action: "disconnect",
      edge: { from: "commander", to: "triage" },
    }));
  });

  test("the drawer says when an idle agent last moved and when its memory was last given", () => {
    const now = Date.now();
    render(
      <TopologyPanel
        view="graph"
        activity={[{ id: "inject", event: "memory.injected", identity: "billing", timestampMs: now - 2 * 60_000, data: {} }]}
        agents={[agent("billing", {
          progress: { health: "healthy", run_state: "idle", in_flight_work: 0, last_progress_at_ms: now - 4 * 60_000, last_progress_event: "became_idle" },
        })]}
        nodes={[queryNode("billing"), queryNode("dispatch")]}
        links={[link("billing", "dispatch", { actual: true, declared: true, desired: true })]}
        memoryScopes={[{ identity: "billing", records: 12 }]}
      />,
    );
    pressNode("billing");
    const drawer = screen.getByTestId("topology-drawer");
    expect(drawer).toHaveTextContent("last active 4m ago");
    expect(drawer).toHaveTextContent("12 records · last given 2m ago");
  });

  test("the overflow menu holds the window, the edge mode and the filters", () => {
    render(<TopologyPanel view="graph" activity={[]} agents={[]} nodes={NODES} management={management} onRequestMutation={vi.fn()} />);
    openMenu();
    expect(screen.getByTestId("topology-edges:all")).toBeInTheDocument();
    expect(screen.getByTestId("topology-edges:focus")).toBeInTheDocument();
    expect(screen.getByTestId("topology-window:15m")).toBeInTheDocument();
    expect(screen.getByTestId("topology-show-helpers")).toBeInTheDocument();
    expect(screen.getByTestId("topology-view:connections")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("topology-edges:focus"));
    // Focus mode: with nothing hovered or selected no edge draws.
    expect(screen.queryByTestId("topology-edge:commander:triage:wired")).toBeNull();
  });

  test("switches the map to clusters above 150 nodes", () => {
    const many = Array.from({ length: 160 }, (_, index) => ({ identity: `agent-${index}`, label: `Agent ${index}`, wired_to: [] }));
    render(<TopologyPanel view="graph" activity={[]} agents={[]} nodes={many} />);
    expect(screen.getByTestId("topology-dense-map")).toHaveAttribute("data-layout", "clusters");
  });
});
