/**
 * The Memory panel inside the console (ConsoleApp over a scripted
 * transport): the experience and the memory reads decide between "not set
 * up", "no access", "withheld by access" and data.
 */
import React from "react";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "../../lib/contract";
import type { MetaConsoleTransport } from "../../lib/headless";

// DOM assertions compare booleans and strings, never elements.

const AGENT = "identity:agent-000";
const NOW = Date.now();

interface MemoryExperience {
  available?: boolean;
  can_read?: boolean;
}

type Reply = (command: string, params: Record<string, unknown> | undefined) => unknown;

function experience(memory: MemoryExperience) {
  return {
    contract_version: "test",
    runtime_id: "memory-runtime",
    storage_scope: "memory-principal",
    console_config: {},
    console_policy: {},
    agent_sidebar: {
      live_snapshot: {
        agents: [
          {
            identity: AGENT,
            member_id: AGENT,
            agent_id: AGENT,
            label: "Agent 0",
            kind: "member",
            role: "worker",
            state: "running",
            addressable: true,
            affordances: {},
            model_capabilities: { image_input: false },
          },
        ],
      },
    },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
    memory,
  };
}

function scripted(memory: MemoryExperience, reply: Reply, methods: string[] = Object.values(CONSOLE_RPC_METHODS)) {
  const commands: string[] = [];
  const transport: MetaConsoleTransport = {
    loadExperience: async () => experience(memory) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => undefined,
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => {
      commands.push(input.command);
      const result = await reply(input.command, input.params as Record<string, unknown> | undefined);
      return { command: input.command, accepted: true, result: result ?? {} } as never;
    },
  };
  return { transport, memoryCommands: () => commands.filter((command) => /memory/i.test(command)) };
}

/// The server's ABAC refusal (-32030).
function refused(): Error {
  return Object.assign(new Error("access denied"), { rpcError: { code: -32030, message: "access denied" } });
}

function record(id: string, title: string) {
  return {
    id,
    title,
    kind: "fact",
    trust: "agent_observed",
    status: { status: "active" },
    scope: { scope: "identity", realm: "ops", identity: AGENT },
    created_at_ms: NOW - 3_600_000,
    updated_at_ms: NOW - 3_600_000,
  };
}

async function openMemory(view: ReturnType<typeof render>): Promise<void> {
  fireEvent.click(await view.findByRole("button", { name: "Memory", exact: true }));
  await view.findByTestId("memory-panel");
}

describe("Memory in the console", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => window.localStorage.clear());

  it("says memory is not set up when the runtime reports no store, and reads nothing", async () => {
    const { transport, memoryCommands } = scripted({ available: false, can_read: false }, () => ({}));
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    try {
      await openMemory(view);
      await view.findByTestId("memory-unavailable");
      const state = view.getByTestId("memory-not-set-up").textContent ?? "";
      expect(state.includes("Memory isn't set up on this runtime.")).toBe(true);
      expect(view.getByText("How to set up memory").closest("a")?.getAttribute("href")?.startsWith("https://")).toBe(true);
      expect(/no access/i.test(view.getByTestId("memory-panel").textContent ?? "")).toBe(false);
      expect(memoryCommands()).toEqual([]);
    } finally {
      view.unmount();
    }
  });

  it("reads a missing records method as not set up only when the runtime reports no store", async () => {
    // An older runtime: no store flag, and no memory methods in its capabilities.
    const methods = Object.values(CONSOLE_RPC_METHODS).filter((method) => !method.startsWith("meta/memory/"));
    const { transport } = scripted({ can_read: true }, () => ({}), methods);
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    try {
      await openMemory(view);
      await view.findByTestId("memory-not-set-up");
      expect(view.queryByText("Records: no access.")).toBeNull();
    } finally {
      view.unmount();
    }
  });

  it("keeps a refused records read as no access, never as not set up", async () => {
    const { transport } = scripted({ available: true, can_read: true }, (command) => {
      if (command === "listMemoryRecords") throw refused();
      return {};
    });
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    try {
      await openMemory(view);
      await waitFor(() => expect((view.getByTestId("memory-panel").textContent ?? "").includes("Records: no access.")).toBe(true));
      expect(view.queryByTestId("memory-not-set-up")).toBeNull();
      expect(view.queryByTestId("memory-unavailable")).toBeNull();
    } finally {
      view.unmount();
    }
  });

  it("shows dream history withheld by access calmly, never as no dream yet", async () => {
    const { transport } = scripted({ available: true, can_read: true }, (command) => {
      if (command === "listMemoryRecords") return { records: [record("mem-1", "Refund approval threshold")], realms: ["ops"], next_cursor: null };
      if (command === "listMemoryDreams") return { runs: [], realms: ["ops"], withheld_by_access: true };
      if (command === "listMemoryDreamRuns") return { runs: [], realms: ["ops"], withheld_by_access: true };
      return {};
    });
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    try {
      await openMemory(view);
      await waitFor(() => expect(view.getByTestId("memory-verdict:dreams").getAttribute("data-status")).toBe("no-grant"));
      expect((view.getByTestId("memory-verdict:dreams").textContent ?? "").includes("Dream history is withheld by access")).toBe(true);
      await act(async () => {
        fireEvent.click(view.getByTestId("memory-tab:dreams"));
      });
      expect((view.getByTestId("memory-dream-runs-withheld").textContent ?? "").includes("Dream history needs access to all memory in this space.")).toBe(true);
      expect(view.queryByTestId("memory-dreams-withheld") === null).toBe(false);
      const text = view.getByTestId("memory-panel").textContent ?? "";
      expect(/no dream (run is recorded|runs recorded|ran this week|has run)/i.test(text)).toBe(false);
    } finally {
      view.unmount();
    }
  });

  it("keeps the empty dream copy when an older server sends no withheld flag", async () => {
    const { transport } = scripted({ available: true, can_read: true }, (command) => {
      if (command === "listMemoryRecords") return { records: [record("mem-1", "Refund approval threshold")], realms: ["ops"], next_cursor: null };
      if (command === "listMemoryDreams" || command === "listMemoryDreamRuns") return { runs: [], realms: ["ops"] };
      return {};
    });
    const view = render(<ConsoleApp baseUrl="" transport={transport} />);
    try {
      await openMemory(view);
      await waitFor(() => expect(view.getByTestId("memory-verdict:dreams").getAttribute("data-status") === "no-grant").toBe(false));
      await act(async () => {
        fireEvent.click(view.getByTestId("memory-tab:dreams"));
      });
      expect(view.queryByTestId("memory-dream-runs-withheld")).toBeNull();
      expect((view.getByTestId("memory-dream-runs").textContent ?? "").includes("No dream run is recorded yet.")).toBe(true);
    } finally {
      view.unmount();
    }
  });
});
