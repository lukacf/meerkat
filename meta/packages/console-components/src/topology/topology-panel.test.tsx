import React from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { topologyEdgeKey, type TopologyManagementState } from "@console-core";

import { TopologyPanel } from "./topology-panel";
import type { ConsoleTopologyNode } from "./types";

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

    expect(screen.queryByTestId("topology-view:connections")).toBeNull();
    expect(screen.getByTestId("topology-panel")).toHaveAttribute("data-management-mode", "disabled");
  });

  test("exposes a controlled search-first connection view from explicit management state", () => {
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
});
