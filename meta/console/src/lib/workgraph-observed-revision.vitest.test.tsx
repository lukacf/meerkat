/**
 * A WorkGraph action acts on the state the viewer saw. Item mutations are
 * compare-and-swap on the item's revision: a card or panel row that shows a
 * revision sends exactly that one, and one that shows none sends nothing and
 * is read again, instead of acting on whatever revision is current at the
 * click (another person's change would then go unnoticed).
 */
import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import { CONSOLE_COMMAND_NAMES, type ConsoleCommandRequest, type ConsoleWhoami, type MetaConsoleTransport } from "./headless";

// DOM assertions compare booleans and strings, never elements.

const NAMESPACE = "runtime/realm/workgraph-observed-revision";
const CAPTURED_AT = "2026-11-06T09:00:00Z";

/** The work graph as the gateway holds it: `item-seen` at revision 4 as
 * listed (another person moved it to 9 since), `item-unseen` listed without
 * its revision. */
function gateway() {
  const commands: Array<{ command: string; params: Record<string, unknown> }> = [];
  const items = [
    { id: "item-seen", title: "Review the plan", status: "open", revision: 4 },
    { id: "item-unseen", title: "Write the report", status: "open" },
  ];
  const executeCommand = vi.fn(async (input: ConsoleCommandRequest) => {
    const params = input.params ?? {};
    commands.push({ command: input.command, params });
    switch (input.command) {
      case CONSOLE_COMMAND_NAMES.workgraphSnapshot:
        return { command: input.command, accepted: true, result: { captured_at: CAPTURED_AT, items, edges: [], attention: [] } };
      case CONSOLE_COMMAND_NAMES.workgraphEvents:
        return { command: input.command, accepted: true, result: { events: [] } };
      case CONSOLE_COMMAND_NAMES.workgraphGet:
        return { command: input.command, accepted: true, result: { item: { id: params.id, status: "open", revision: 9 } } };
      case CONSOLE_COMMAND_NAMES.workgraphClaim:
        return { command: input.command, accepted: true, result: { item: { id: params.id, status: "in_progress", revision: 10 } } };
      default:
        return { command: input.command, accepted: true, result: {} };
    }
  });
  const transport: MetaConsoleTransport = {
    loadExperience: async () => ({
      runtime_id: "workgraph-observed-revision",
      console_config: { layout: { initial_control: "workgraph" } },
      workgraph: { available: true, can_view: true, can_manage: true },
      agent_sidebar: { live_snapshot: { agents: [] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({
      version: "test",
      methods: [
        "meta/console/timeline",
        CONSOLE_RPC_METHODS.workgraphSnapshot,
        CONSOLE_RPC_METHODS.workgraphEvents,
        CONSOLE_RPC_METHODS.workgraphGet,
        CONSOLE_RPC_METHODS.workgraphGoalStatus,
        CONSOLE_RPC_METHODS.workgraphClaim,
        CONSOLE_RPC_METHODS.workgraphRelease,
        CONSOLE_RPC_METHODS.workgraphClose,
      ],
    }) as never,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {},
    send: vi.fn(),
    // A signed-in person: the console offers Claim only to a caller whoami names.
    whoami: async () => ({ actorKind: "person", principal: "ops@example.test", approverIds: [], canAdminister: false }) as ConsoleWhoami,
    executeCommand: executeCommand as never,
    upload: async () => ({ blob_id: "blob" }) as never,
    blobUrl: (id) => `/blobs/${id}`,
  };
  return { transport, commands };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});

afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("a WorkGraph action", () => {
  it("sends the revision its row showed, and none for a row that showed none", async () => {
    const { transport, commands } = gateway();
    render(<ConsoleApp baseUrl="" storageNamespace={NAMESPACE} transport={transport} />);
    await screen.findByTestId("workgraph-panel-action:item-seen:claim");

    // The row showed revision 4: the claim names 4, read at no click.
    fireEvent.click(screen.getByTestId("workgraph-panel-action:item-seen:claim"));
    await waitFor(() => expect(commands.some((entry) => entry.command === CONSOLE_COMMAND_NAMES.workgraphClaim)).toBe(true));
    const claims = commands.filter((entry) => entry.command === CONSOLE_COMMAND_NAMES.workgraphClaim);
    expect(claims.map((entry) => [entry.params.id, entry.params.expected_revision])).toEqual([["item-seen", 4]]);
    expect(commands.some((entry) => entry.command === CONSOLE_COMMAND_NAMES.workgraphGet)).toBe(false);

    // The row showed no revision: nothing is claimed, no revision is read
    // to claim with, and the viewer is told to look again.
    const before = commands.length;
    fireEvent.click(await screen.findByTestId("workgraph-panel-action:item-unseen:claim"));
    await waitFor(() => expect(screen.queryByTestId("console-action-error") !== null).toBe(true));
    const after = commands.slice(before);
    expect(after.some((entry) => entry.command === CONSOLE_COMMAND_NAMES.workgraphClaim)).toBe(false);
    expect(after.some((entry) => entry.command === CONSOLE_COMMAND_NAMES.workgraphGet)).toBe(false);
    expect(screen.getByTestId("console-action-error").textContent?.includes("nothing was changed")).toBe(true);
  });
});
