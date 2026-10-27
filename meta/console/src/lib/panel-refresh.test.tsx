/**
 * Panel data refreshes are event-driven. On a server slower than the event
 * rate they used to overlap without limit: every tool or lifecycle frame from
 * any agent re-queried topology (and every other docked panel kind), even for
 * panels in hidden tabs, until the browser's per-origin connection limit
 * starved every other console request. These pin the contract: hidden panels
 * are not refreshed, and a visible panel has at most one request in flight,
 * with requests made meanwhile coalesced into one trailing refresh.
 */
import React from "react";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { buildControlTarget } from "./adapters";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { MetaConsoleTransport } from "./headless";
import type { ConsoleFrame } from "../types";
import { savePanelRuntimeSelection } from "./panel-runtime";

const CHAT = "identity:agent-000";

function experience() {
  return {
    contract_version: "test",
    runtime_id: "refresh-runtime",
    storage_scope: "refresh-principal",
    console_config: {},
    console_policy: {},
    agent_sidebar: {
      live_snapshot: {
        agents: [0, 1].map((i) => ({
          identity: `identity:agent-00${i}`,
          member_id: `identity:agent-00${i}`,
          agent_id: `identity:agent-00${i}`,
          label: `Agent ${i}`,
          kind: "member",
          role: "worker",
          state: "running",
          addressable: true,
          affordances: {},
          model_capabilities: { image_input: false },
        })),
      },
    },
    activity_feed: { filter_presets: [], active_preset_id: "all" },
  };
}

interface Harness extends MetaConsoleTransport {
  live?: (frame: ConsoleFrame) => void;
  topologyQueries: number;
  inFlight: number;
  maxInFlight: number;
  release: Array<() => void>;
}

function transport(): Harness {
  const harness: Harness = {
    topologyQueries: 0,
    inFlight: 0,
    maxInFlight: 0,
    release: [],
    loadExperience: async () => experience() as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: Object.values(CONSOLE_RPC_METHODS) }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: (_input, onFrame) => {
      harness.live = onFrame;
      return () => { harness.live = undefined; };
    },
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => {
      if (input.command === "topologyQuery") {
        harness.topologyQueries += 1;
        harness.inFlight += 1;
        harness.maxInFlight = Math.max(harness.maxInFlight, harness.inFlight);
        // A server that answers only when the test says so.
        await new Promise<void>((resolve) => harness.release.push(resolve));
        harness.inFlight -= 1;
        return { command: input.command, accepted: true, result: { members: [], edges: [] } } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
  return harness;
}

function seedDock(topologyVisible: boolean): void {
  const topology = { id: "panel-1", mode: "console", target: buildControlTarget("topology") };
  const chat = {
    id: "panel-2",
    mode: "console",
    target: { id: `chat:${CHAT}`, kind: "agent-chat", title: "Agent 0", identity: CHAT, memberId: CHAT },
  };
  window.localStorage.setItem(`meta-console-dock-state:${JSON.stringify(["", "refresh-principal"])}`, JSON.stringify({
    tabs: [
      { id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } },
      { id: "tab-2", presetId: "single", layout: { kind: "panel", panelId: "panel-2" } },
    ],
    panels: [topology, chat],
    activeTabId: topologyVisible ? "tab-1" : "tab-2",
    focusedPanelId: topologyVisible ? "panel-1" : "panel-2",
  }));
}

async function wait(ms: number): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

/// Tool activity from another agent: each frame schedules a panel refresh.
async function agentActivity(harness: Harness, frames: number): Promise<void> {
  for (let i = 0; i < frames; i += 1) {
    await act(async () => {
      harness.live?.({
        id: `tool:${Date.now()}:${i}`,
        event: "tool_call_requested",
        identity: "identity:agent-001",
        interactionId: "busy",
        timestampMs: Date.now(),
        cursor: `console:${1000 + i}`,
        data: { id: `call-${i}`, name: "read_file" },
      });
    });
    await wait(200);
  }
}

describe("event-driven panel refresh", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => window.localStorage.clear());

  it("never queries topology for a panel in a hidden tab", async () => {
    seedDock(false);
    const harness = transport();
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await wait(300);
    await agentActivity(harness, 12);
    expect(harness.topologyQueries).toBe(0);
    view.unmount();
  }, 30_000);

  it("keeps one topology query in flight and coalesces events into trailing queries", async () => {
    seedDock(true);
    const harness = transport();
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await wait(300);
    await agentActivity(harness, 12);
    expect(harness.maxInFlight).toBe(1);
    expect(harness.topologyQueries).toBe(1);
    // The slow answers arrive one by one. The events that arrived meanwhile
    // collapse into trailing refreshes (one per coalescing level: the
    // event-driven refresh and the topology refresh), never overlapping, and
    // the queries stop once the backlog is answered.
    for (let answered = 0; answered < 5 && harness.release.length; answered += 1) {
      await act(async () => { harness.release.shift()?.(); });
      await wait(300);
    }
    expect(harness.release).toHaveLength(0);
    expect(harness.maxInFlight).toBe(1);
    expect(harness.topologyQueries).toBeLessThanOrEqual(3);
    const settled = harness.topologyQueries;
    await wait(600);
    expect(harness.topologyQueries).toBe(settled);
    view.unmount();
  }, 30_000);
});

describe("empty shared-console panels", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => window.localStorage.clear());

  function emptyExperience() {
    return {
      ...experience(),
      agent_sidebar: { live_snapshot: { agents: [] } },
      panel_runtimes: [],
      memory: { available: false, can_read: false },
      workgraph: { available: false, can_view: false, can_manage: false },
    };
  }

  it("keeps Memory and WorkGraph discoverable without querying a nonexistent team", async () => {
    const harness = transport();
    harness.loadExperience = async () => emptyExperience() as never;
    const execute = vi.fn(harness.executeCommand);
    harness.executeCommand = execute;
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    const memory = await view.findByRole("button", { name: "Memory", exact: true });
    fireEvent.click(memory);
    expect(await view.findByText("No teams are running.")).toBeTruthy();
    expect(view.getByText("Finish setting up a team to inspect its memory and work graph.")).toBeTruthy();
    expect(view.queryByRole("combobox", { name: "Memory mob" })).toBeNull();
    fireEvent.click(view.getByRole("button", { name: "WorkGraph", exact: true }));
    expect(await view.findByRole("heading", { name: "WorkGraph", exact: true })).toBeTruthy();
    expect(view.getByText("No teams are running.")).toBeTruthy();
    expect(execute.mock.calls.filter(([input]) => /memory|workgraph/i.test(input.command))).toHaveLength(0);
    view.unmount();
  });

  it.each(["Memory", "WorkGraph"])("opens %s data when the saved team becomes available", async (name) => {
    savePanelRuntimeSelection(window.localStorage, JSON.stringify(["", "refresh-principal"]), "alpha");
    let started = false;
    const harness = transport();
    harness.loadExperience = async () => (started ? {
      ...experience(),
      panel_runtimes: [{ key: "alpha", label: "Alpha", namespace: "alpha", memory: true, workgraph: true }],
      memory: { available: true, can_read: true },
      workgraph: { available: true, can_view: true, can_manage: false },
    } : emptyExperience()) as never;
    const execute = vi.fn(harness.executeCommand);
    harness.executeCommand = execute;
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    fireEvent.click(await view.findByRole("button", { name, exact: true }));
    expect(await view.findByText("No teams are running.")).toBeTruthy();
    const queries = () => execute.mock.calls.filter(([input]) => /memory|workgraph/i.test(input.command));
    expect(queries()).toHaveLength(0);
    started = true;
    await act(async () => harness.live?.({
      id: `ready:${name}`, event: "member_ready", identity: CHAT,
      timestampMs: Date.now(), cursor: "console:2000", data: {},
    }));
    await waitFor(() => expect(queries().length).toBeGreaterThan(0));
    expect(view.queryByText("No teams are running.")).toBeNull();
    expect(view.getByRole("combobox", { name: name === "Memory" ? "Memory mob" : "Work Graph mob" })).toHaveValue("alpha");
    for (const [input] of queries()) expect(input.params?.runtime_key).toBe("alpha");
    view.unmount();
  });

  it.each([undefined, [{ key: "restricted", label: "Restricted", namespace: "restricted", memory: true, workgraph: true }]])(
    "does not replace missing capability information or denied access with a setup explanation (%j)",
    async (runtimes) => {
      const harness = transport();
      harness.loadExperience = vi.fn(async () => ({
        ...emptyExperience(), panel_runtimes: runtimes,
        memory: { available: true, can_read: false },
        workgraph: { available: true, can_view: false, can_manage: false },
      }) as never);
      const view = render(<ConsoleApp baseUrl="" transport={harness} />);
      await waitFor(() => expect(harness.loadExperience).toHaveBeenCalled());
      expect(view.queryByRole("button", { name: "Memory", exact: true })).toBeNull();
      expect(view.queryByRole("button", { name: "WorkGraph", exact: true })).toBeNull();
      expect(view.queryByText("No teams are running.")).toBeNull();
      view.unmount();
    },
  );
});

describe("manual memory detail refresh", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => window.localStorage.clear());

  it("refreshes usage in the open record and does not reopen it after Back", async () => {
    const harness = transport();
    harness.loadExperience = async () => ({
      ...experience(),
      panel_runtimes: [
        { key: "alpha", label: "Alpha", namespace: "alpha", memory: true, workgraph: false },
        { key: "beta", label: "Beta", namespace: "beta", memory: true, workgraph: false },
      ],
      memory: { available: true, can_read: true },
    }) as never;
    let count = 0;
    let listQueries = 0;
    let detailError = false;
    let releaseDetail: (() => void) | undefined;
    let delayDetail = false;
    const detailParams: unknown[] = [];
    const record = () => ({
      id: "refresh-record",
      title: "Refresh fixture",
      scope: { scope: "realm", realm: "fixture-realm" },
      kind: "fact",
      trust: "operator",
      status: { status: "active" },
      usage: { injected_count: count, explicit_recall_count: count },
    });
    harness.executeCommand = async (input) => {
      let result = {};
      if (input.command === "listMemoryRecords") {
        listQueries += 1;
        result = { records: [record()], next_cursor: null, realms: ["fixture-realm"] };
      } else if (input.command === "getMemoryRecord") {
        detailParams.push(input.params);
        if (delayDetail) await new Promise<void>((resolve) => { releaseDetail = resolve; });
        if (detailError) throw new Error("record refresh unavailable");
        result = { realm: "fixture-realm", record: { ...record(), body: "Synthetic fixture." }, chain: [], injections: [] };
      }
      return { command: input.command, accepted: true, result } as never;
    };
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    try {
      fireEvent.click(await view.findByRole("button", { name: "Memory", exact: true }));
      fireEvent.click(await view.findByTestId("memory-tab:records"));
      fireEvent.click(await view.findByTestId("memory-record:refresh-record"));
      await waitFor(() => expect(view.getByTestId("memory-detail")).toHaveTextContent(/injected 0.*recalled 0/));
      expect(detailParams).toHaveLength(1);

      count = 1;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(view.getByTestId("memory-detail")).toHaveTextContent(/injected 1.*recalled 1/));
      expect(detailParams).toHaveLength(2);
      expect(detailParams[1]).toMatchObject({ runtime_key: "alpha", realm: "fixture-realm", memory_id: "refresh-record" });

      fireEvent.click(view.getByTestId("memory-tab:holdings"));
      const beforeOverviewRefresh = listQueries;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(listQueries).toBeGreaterThan(beforeOverviewRefresh));
      expect(detailParams).toHaveLength(2);
      fireEvent.click(view.getByTestId("memory-tab:records"));

      detailError = true;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(view.getByTestId("memory-error")).toHaveTextContent("record refresh unavailable"));
      expect(view.getByTestId("memory-detail")).toHaveTextContent(/injected 1.*recalled 1/);
      expect(view.getByRole("combobox", { name: "Memory mob" })).toHaveValue("alpha");

      detailError = false;
      count = 2;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(view.getByTestId("memory-detail")).toHaveTextContent(/injected 2.*recalled 2/));
      expect(view.queryByTestId("memory-error")).toBeNull();

      count = 3;
      delayDetail = true;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(releaseDetail).toBeDefined());
      fireEvent.click(view.getByTestId("memory-tab:holdings"));
      await act(async () => releaseDetail?.());
      expect(view.getByTestId("memory-tab:holdings")).toHaveClass("is-active");
      fireEvent.click(view.getByTestId("memory-tab:records"));
      expect(view.getByTestId("memory-detail")).toHaveTextContent(/injected 3.*recalled 3/);

      releaseDetail = undefined;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(releaseDetail).toBeDefined());
      fireEvent.click(view.getByTestId("memory-detail-back"));
      await act(async () => releaseDetail?.());
      expect(view.queryByTestId("memory-detail")).toBeNull();
      const beforeListRefresh = listQueries;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(listQueries).toBeGreaterThan(beforeListRefresh));
      expect(detailParams).toHaveLength(6);
      expect(view.queryByTestId("memory-detail")).toBeNull();

      delayDetail = false;
      fireEvent.click(await view.findByTestId("memory-record:refresh-record"));
      await waitFor(() => expect(view.getByTestId("memory-detail")).toHaveTextContent(/injected 3.*recalled 3/));
      delayDetail = true;
      releaseDetail = undefined;
      fireEvent.click(view.getByTestId("memory-refresh"));
      await waitFor(() => expect(releaseDetail).toBeDefined());
      fireEvent.change(view.getByRole("combobox", { name: "Memory mob" }), { target: { value: "beta" } });
      expect(view.queryByTestId("memory-detail")).toBeNull();
      fireEvent.change(view.getByRole("combobox", { name: "Memory mob" }), { target: { value: "alpha" } });
      await act(async () => releaseDetail?.());
      await view.findByTestId("memory-tab:holdings");
      expect(view.queryByTestId("memory-detail")).toBeNull();
      expect(view.getByRole("combobox", { name: "Memory mob" })).toHaveValue("alpha");
    } finally {
      view.unmount();
    }
  });
});
