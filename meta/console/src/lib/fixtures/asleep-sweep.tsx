// The shared body of the two sweep rows (hosted-mob-asleep-sweep-*): every
// pane the console has, one at a time, on a gateway that offers every
// method, with a hosted root that is asleep. The console is left to itself
// while everything that makes it act on its own happens, and whatever the
// pane, no request names the sleeping root and the root is not woken.

import React from "react";
import { act, render, screen, waitFor } from "@testing-library/react";
import { expect, vi } from "vitest";

import { ConsoleApp } from "../../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "../contract";
import { createTwoRootGateway, HOSTED_MOB, type RecordedRequest, type TwoRootGateway } from "./two-root-gateway";

export const SWEEP_MEMBER = "support/reviewer";
const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';

/** Every pane: the two of a member, and each control panel. */
export const SWEEP_PANES: Array<{ name: string; target: Record<string, unknown> }> = [
  { name: "a chat with the root's member", target: { id: SWEEP_MEMBER, kind: "agent-chat", addressingMode: "identity", memberId: SWEEP_MEMBER, identity: SWEEP_MEMBER, title: "Support reviewer" } },
  { name: "the Details of the root's member", target: { id: `inspect:${SWEEP_MEMBER}`, kind: "identity-inspect", identity: SWEEP_MEMBER, memberId: SWEEP_MEMBER, title: "Support reviewer Details" } },
  ...["timeline", "roster", "topology", "health", "routing", "gating", "access", "memory", "workgraph", "schedules", "logs"].map((kind) => ({
    name: `the ${kind} panel`,
    target: { id: kind, kind, title: kind },
  })),
];

const settle = (ms: number) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

/**
 * The pane under test is the one on screen: the one pane of the dock shows
 * the saved target. Without this a row would pass with nothing open.
 */
function expectPaneOnScreen(target: Record<string, unknown>) {
  expect(document.querySelectorAll("[data-testid^='pane:']").length).toBe(1);
  const title = screen.getByTestId("pane-title:panel-1").textContent || "";
  expect(title).toBe(String(target.title));
  // The pane drew a body under its bar.
  expect((screen.getByTestId("pane:panel-1").textContent || "").length).toBeGreaterThan(title.length);
}

function savePane(target: Record<string, unknown>) {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", target, mode: "console" }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
}

/** A gateway that offers every method the contract names and every pane. */
const gatewayWithEverything = (hostedState: "running" | "dormant") => createTwoRootGateway({
  hostedState,
  methods: Object.values(CONSOLE_RPC_METHODS),
  experience: {
    access: { enabled: false, can_administer: true },
    memory: { available: true },
    workgraph: { available: true, can_manage: true },
    runtime_capabilities: { can_send_messages: true, can_spawn_members: true, can_retire_members: true, can_wire_members: true },
  },
});

/** The requests that name one mob (`mob_id`). A read that asks for every mob (`mobs`) names none. */
const namingOneMob = (requests: RecordedRequest[]) =>
  requests.filter((request) => "mob_id" in (request.params ?? {}) || request.query.some(([name]) => name === "mob_id"))
    .map((request) => [request.method ?? request.route, request.params?.mob_id]);

/**
 * Everything that makes the console act on its own: frames of the launch
 * mob that ask for a refresh, a frame of the sleeping root's member, memory
 * and work signals, a gap in the stream, and time (the console's own delays
 * are 150 to 250 ms).
 */
async function leftToItself(gateway: TwoRootGateway) {
  act(() => {
    gateway.push({ identity: "planner", kind: "state_changed", payload: {} });
    gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_complete", interactionId: "while-asleep", payload: {} });
    gateway.push({ identity: "planner", kind: "memory.record_written", payload: {} });
    gateway.push({ identity: "planner", kind: "workgraph.item_updated", payload: {} });
    gateway.push({ identity: "planner", kind: "topology_updated", payload: {} });
  });
  await settle(450);
  act(() => { gateway.gap(); });
  await settle(450);
  act(() => { gateway.push({ identity: "planner", kind: "member_ready", payload: {} }); });
  await settle(550);
}

/** The pane restored on a root that is asleep when the page loads. */
export async function sweepRestoredOnASleepingRoot(target: Record<string, unknown>): Promise<void> {
  savePane(target);
  const gateway = gatewayWithEverything("dormant");
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  expect((await screen.findByTestId("sidebar-mob-state:support")).textContent).toBe("Asleep");
  await settle(500);
  expectPaneOnScreen(target);
  await leftToItself(gateway);
  expectPaneOnScreen(target);
  expect(gateway.requests.length).toBeGreaterThan(5);
  expect(namingOneMob(gateway.requests)).toEqual([]);
  expect(gateway.wakes()).toBe(0);
  expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep");
}

/** The pane left open while a loaded root goes to sleep. */
export async function sweepOpenWhileTheRootGoesToSleep(target: Record<string, unknown>): Promise<void> {
  savePane(target);
  const gateway = gatewayWithEverything("running");
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId(`sidebar-agent:${SWEEP_MEMBER}`);
  await settle(700);

  gateway.set({ hostedState: "dormant" });
  act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
  await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 5_000 });
  await settle(450);
  gateway.clear();
  const wakesBefore = gateway.wakes();
  expectPaneOnScreen(target);
  await leftToItself(gateway);
  expectPaneOnScreen(target);
  expect(gateway.requests.length).toBeGreaterThan(2);
  expect(namingOneMob(gateway.requests)).toEqual([]);
  expect(gateway.wakes()).toBe(wakesBefore);
  expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep");
}
