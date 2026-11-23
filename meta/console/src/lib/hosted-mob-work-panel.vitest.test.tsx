import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import { createTwoRootGateway, ensureAbortSignalAny, type FakeWorkItem, type TwoRootGateway } from "./fixtures/two-root-gateway";

// The docked Work panel shows the work graph of the launch mob's door. Its
// reassign takes a typed agent name, and a work item is reassigned within
// the mob whose graph the panel shows: an agent of a hosted root typed
// there is refused by the page, with words that say why, and no request
// leaves; a launch member typed there goes out with no mob.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';
const GOAL: FakeWorkItem = { id: "goal-1", title: "Ship the release", status: "in_progress", revision: 4 };
const BINDING = "b-coord";
const REASSIGN = CONSOLE_RPC_METHODS.workgraphAttentionReassign;

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

/** A gateway whose launch door's work graph holds a goal with a coordinating binding, and the Work panel as the one pane. */
async function startWithTheWorkPanel(): Promise<TwoRootGateway> {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", target: { id: "workgraph", kind: "workgraph", title: "workgraph" }, mode: "console" }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
  const gateway = createTwoRootGateway({
    methods: Object.values(CONSOLE_RPC_METHODS),
    experience: { workgraph: { available: true, can_view: true, can_manage: true } },
    workGraph: { launch: [{ ...GOAL }], hosted: [] },
    launchAttention: [{
      binding_id: BINDING,
      mode: "coordinate",
      item_id: GOAL.id,
      status: { state: "active" },
      machine_state: { revision: 7 },
      target: { kind: "identity", identity: "planner" },
    }],
  });
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId(`sidebar-agent:support/reviewer`);
  await screen.findByTestId(`workgraph-panel-binding:${BINDING}`, undefined, { timeout: 6_000 });
  await settle();
  return gateway;
}

/** Type `identity` as the binding's new owner in the panel and submit it. */
async function reassignTo(identity: string) {
  const row = screen.getByTestId(`workgraph-panel-binding:${BINDING}`);
  const open = [...row.querySelectorAll("button")].find((button) => button.textContent === "Reassign");
  expect(open, "the binding offers a reassign").toBeTruthy();
  await act(async () => { fireEvent.click(open!); });
  const input = await screen.findByTestId(`workgraph-panel-reassign-input:${BINDING}`);
  await act(async () => { fireEvent.change(input, { target: { value: identity } }); });
  await act(async () => { fireEvent.click(screen.getByTestId(`workgraph-panel-reassign-submit:${BINDING}`)); });
  await settle();
}

beforeEach(() => {
  ensureAbortSignalAny();
  window.localStorage.clear();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  window.localStorage.clear();
});

describe("the docked Work panel on a gateway that hosts a second root", () => {
  it("refuses a hosted root's agent typed as a binding's new owner, in words that say why, and sends nothing", async () => {
    const gateway = await startWithTheWorkPanel();
    gateway.clear();
    await reassignTo("support/reviewer");
    const said = (await screen.findByTestId("console-action-error")).textContent || "";
    expect(said).toContain("The Work panel shows the work of ");
    expect(said).toContain("support/reviewer is an agent of another team, and a work item is reassigned within its own team. Nothing was sent.");
    expect(gateway.calls(REASSIGN)).toEqual([]);
    expect(gateway.requests.filter((request) => request.params?.mob_id !== undefined)).toEqual([]);
  }, 20_000);

  it("sends a launch member typed as a binding's new owner to the launch mob's door, with no mob", async () => {
    const gateway = await startWithTheWorkPanel();
    gateway.clear();
    await reassignTo("reviewer");
    await waitFor(() => expect(gateway.calls(REASSIGN).length).toBe(1), { timeout: 6_000 });
    const sent = gateway.calls(REASSIGN)[0]?.params as Record<string, unknown>;
    expect(sent).toMatchObject({ binding_id: BINDING, expected_revision: 7, target: { kind: "identity", identity: "reviewer" } });
    expect(Object.keys(sent)).not.toContain("mob_id");
  }, 20_000);
});
