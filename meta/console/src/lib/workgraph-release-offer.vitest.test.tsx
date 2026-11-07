/**
 * Who the stock console offers WorkGraph claims and releases to. The gateway
 * owns a claim by its caller and releases it for the claim's owner or for an
 * access admin, so the console offers Claim only to a caller with a
 * principal, and Release on a claim only to its owner or to an access admin.
 */
import React from "react";
import { fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { ConsoleWhoami, MetaConsoleTransport } from "./headless";

// DOM assertions compare booleans and strings, never elements.

const OWNER = "owner@example.test";
const AT = "2026-11-06T09:00:00Z";

function experience() {
  return {
    contract_version: "test",
    runtime_id: "workgraph-runtime",
    storage_scope: "workgraph-principal",
    console_config: {},
    console_policy: {},
    agent_sidebar: { live_snapshot: { agents: [] } },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
    workgraph: { available: true, can_view: true, can_manage: true },
  };
}

// One item in progress under someone's claim, one open item nobody owns.
const items = [
  {
    id: "claimed-item", title: "Claimed item", status: "in_progress", revision: 4, created_at: AT, updated_at: AT,
    claim: { owner: { key: { kind: "principal", id: OWNER } }, claimed_at: AT },
  },
  { id: "open-item", title: "Open item", status: "open", revision: 1, created_at: AT, updated_at: AT },
];

function transport(whoami: ConsoleWhoami): MetaConsoleTransport {
  return {
    loadExperience: async () => experience() as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => {},
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    whoami: async () => whoami,
    executeCommand: async (input) => {
      if (input.command === "workgraphSnapshot") {
        return { command: input.command, accepted: true, result: { items, edges: [], attention: [], captured_at: AT } } as never;
      }
      if (input.command === "workgraphEvents") {
        return { command: input.command, accepted: true, result: { events: [] } } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
}

function caller(actorKind: string, principal?: string, canAdminister = false): ConsoleWhoami {
  return { actorKind, ...(principal ? { principal } : {}), approverIds: [], canAdminister } as ConsoleWhoami;
}

/** The console with the WorkGraph view open on its outline, its items listed. */
async function workGraphView(whoami: ConsoleWhoami) {
  const view = render(<ConsoleApp baseUrl="" transport={transport(whoami)} storageNamespace="workgraph" />);
  fireEvent.click(await view.findByRole("button", { name: "WorkGraph", exact: true }));
  // The panel remembers the last view chosen in this process: start on the outline.
  fireEvent.click(await view.findByTestId("workgraph-view-toggle:tree"));
  await view.findByTestId("workgraph-panel-item:open-item");
  return view;
}

/** Whether the claimed item's drawer, in the graph, offers Release. */
async function releaseOffered(whoami: ConsoleWhoami): Promise<boolean> {
  const view = await workGraphView(whoami);
  fireEvent.click(view.getByTestId("workgraph-view-toggle:graph"));
  const node = await waitFor(() => {
    const found = Array.from(view.container.querySelectorAll('[data-testid="workgraph-graph-node"]'))
      .find((element) => element.getAttribute("data-item-id") === "claimed-item");
    if (!found) throw new Error("the claimed item's node");
    return found;
  });
  // The graph selects on a pointer press and release, as a mouse click does.
  fireEvent.pointerDown(node, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
  fireEvent.pointerUp(node, { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
  await view.findByTestId("workgraph-graph-detail");
  const offered = view.queryByTestId("workgraph-panel-action:claimed-item:release") !== null;
  view.unmount();
  return offered;
}

describe("WorkGraph claims and releases in the stock console", () => {
  beforeEach(() => {
    window.localStorage.clear();
    // jsdom has no canvas; text measures fall back to their estimates.
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
  });
  afterEach(() => {
    window.localStorage.clear();
    vi.restoreAllMocks();
  });

  it("offers Release on someone else's claim to an access admin", async () => {
    expect(await releaseOffered(caller("person", "admin@example.test", true))).toBe(true);
  });

  it.each([
    ["the claim's owner", caller("person", OWNER), true],
    ["another signed-in person", caller("person", "carol@example.test"), false],
    ["a caller without a token", caller("anonymous"), false],
  ] as const)("offers Release to %s only on its own claim", async (_label, whoami, offered) => {
    expect(await releaseOffered(whoami)).toBe(offered);
  });

  it.each([
    ["a signed-in person", caller("person", OWNER), true],
    ["a service", caller("service", "svc:robot"), true],
    ["a caller without a token", caller("anonymous"), false],
  ] as const)("offers Claim to %s only with a principal", async (_label, whoami, offered) => {
    const view = await workGraphView(whoami);
    // The whoami read settles after the first render.
    await waitFor(() => expect(view.queryByTestId("workgraph-panel-action:open-item:close") !== null).toBe(true));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(view.queryByTestId("workgraph-panel-action:open-item:claim") !== null).toBe(offered);
    view.unmount();
  });
});
