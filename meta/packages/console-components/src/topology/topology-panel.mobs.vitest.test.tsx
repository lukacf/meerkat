import React from "react";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { TopologyPanel, type TopologyMobChoice } from "./topology-panel";
import type { ConsoleTopologyNode } from "./types";

const NODES: ConsoleTopologyNode[] = [
  { identity: "planner", label: "Planner", role: "coordination", wired_to: ["researcher"], labels: {} },
  { identity: "researcher", label: "Researcher", role: "analysis", wired_to: ["planner"], labels: {} },
];

const localStorageValues = new Map<string, string>();
const localStorageStub = {
  clear: () => localStorageValues.clear(),
  getItem: (key: string) => localStorageValues.get(key) ?? null,
  key: (index: number) => Array.from(localStorageValues.keys())[index] ?? null,
  get length() { return localStorageValues.size; },
  removeItem: (key: string) => { localStorageValues.delete(key); },
  setItem: (key: string, value: string) => { localStorageValues.set(key, value); },
};

const choice = (selected: string | null, onSelect = vi.fn()): TopologyMobChoice => ({
  mobs: [
    { mobId: "ops-team", name: "Ops team", depth: 0 },
    { mobId: "research", name: "Research team", depth: 1 },
    { mobId: "library", name: "Library team", depth: 0 },
  ],
  selected,
  onSelect,
});

describe("TopologyPanel mob choice", () => {
  beforeEach(() => {
    Object.defineProperty(window, "localStorage", { configurable: true, value: localStorageStub });
    localStorageStub.clear();
  });

  test("offers All mobs and each mob in its More actions menu, and reports the choice", () => {
    const onSelect = vi.fn();
    const { rerender } = render(<TopologyPanel activity={[]} agents={[]} nodes={NODES} mobChoice={choice(null, onSelect)} />);
    fireEvent.click(screen.getByTestId("topology-menu"));
    const menu = screen.getByRole("menu");
    const all = within(menu).getByTestId("topology-mob:all");
    expect(all.getAttribute("aria-checked")).toBe("true");
    expect(all.textContent).toContain("All mobs");
    const research = within(menu).getByTestId("topology-mob:research");
    expect(research.textContent).toContain("Research team");
    expect(research.textContent).not.toContain("research-");
    fireEvent.click(research);
    expect(onSelect).toHaveBeenLastCalledWith("research");
    // The host applies the choice; the menu stays open on it.
    rerender(<TopologyPanel activity={[]} agents={[]} nodes={NODES} mobChoice={choice("research", onSelect)} />);
    const reopened = screen.getByRole("menu");
    expect(within(reopened).getByTestId("topology-mob:research").getAttribute("aria-checked")).toBe("true");
    fireEvent.click(within(reopened).getByTestId("topology-mob:all"));
    expect(onSelect).toHaveBeenLastCalledWith(null);
  });

  test("names the chosen mob, or all mobs, in its scope", () => {
    const { rerender } = render(
      <TopologyPanel activity={[]} agents={[]} nodes={NODES} mobChoice={choice("research")} teamName="Research team" teamKey="research" />,
    );
    expect(screen.getByTestId("topology-mob-choice").textContent).toBe("Research team");
    rerender(<TopologyPanel activity={[]} agents={[]} nodes={NODES} mobChoice={choice(null)} teamName="Ops team" teamKey="ops-team" />);
    expect(screen.getByTestId("topology-mob-choice").textContent).toBe("All mobs");
  });

  test("shows no mob choice for a console with one mob", () => {
    render(<TopologyPanel activity={[]} agents={[]} nodes={NODES} teamName="Ops team" teamKey="ops-team" />);
    fireEvent.click(screen.getByTestId("topology-menu"));
    expect(within(screen.getByRole("menu")).queryByTestId("topology-mob:all")).toBeNull();
    expect(screen.queryByTestId("topology-mob-choice")).toBeNull();
  });
});
