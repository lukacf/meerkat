import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type TwoRootGateway } from "./fixtures/two-root-gateway";

// The actions a chat offers for an agent follow what its row says the
// gateway does for it. A hosted root's member whose row says the gateway
// stops its runs and does not retire, respawn, reset or fork it is offered
// Stop while it runs, and none of the others. A launch mob's member whose
// row allows them all is offered them all, as before.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const NAMESPACE = JSON.stringify(["", "console-storage:v1:two-root"]);
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

/** The layout the browser saved: one pane, the chat of `member`. */
function saveChatPane(member: string, title: string) {
  window.localStorage.setItem(`meta-console-dock-state:${NAMESPACE}`, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
    panels: [{ id: "panel-chat", target: { id: member, kind: "agent-chat", addressingMode: "identity", memberId: member, identity: member, title }, mode: "console" }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-chat",
  }));
}

async function start(gateway: TwoRootGateway, member: string) {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId(`chat-composer:${member}`);
  await settle(900);
}

/** A run of the member `identity` of the door `mob` (none: the launch mob) starts. */
const runStarts = (gateway: TwoRootGateway, identity: string, runId: string, mob?: string) => act(() => {
  gateway.push({ ...(mob ? { mob } : {}), identity, kind: "run_started", interactionId: `interaction-${runId}`, payload: { identity: { run_id: runId } }, extra: { run_id: runId } });
});

const EVERY_ACTION = { can_retire: true, can_respawn: true, can_reset: true, can_fork_side_chat: true, can_stop_run: true };

let restoreAbortSignal = () => {};
beforeEach(() => {
  restoreAbortSignal = ensureAbortSignalAny();
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
  cleanup(); vi.unstubAllGlobals(); restoreAbortSignal();
});

describe("the actions of a chat follow the agent's row", () => {
  it("offers a hosted root's member Stop while it runs, and no retire, respawn or fork its row does not allow", async () => {
    const member = "support/reviewer";
    saveChatPane(member, "Support reviewer");
    const gateway = createTwoRootGateway({
      affordances: { launch: EVERY_ACTION, hosted: { can_retire: false, can_respawn: false, can_reset: false, can_fork_side_chat: false, can_stop_run: true } },
    });
    await start(gateway, member);
    runStarts(gateway, "reviewer", "run-hosted-1", HOSTED_MOB);
    await waitFor(() => expect(screen.queryByTestId("conv-action:stop-run")).not.toBeNull(), { timeout: 4_000 });
    expect(screen.queryByTestId("conv-menu"), "no respawn and no retire: the chat has no menu").toBeNull();
    expect(screen.queryByTestId("conv-action:fork-side-chat")).toBeNull();
    expect(screen.queryByTestId("conv-action:details")).not.toBeNull();
  }, 20_000);

  it("offers a launch mob's member every action its row allows, Stop while it runs", async () => {
    saveChatPane("reviewer", "Launch reviewer");
    const gateway = createTwoRootGateway({ affordances: { launch: EVERY_ACTION } });
    await start(gateway, "reviewer");
    expect(screen.queryByTestId("conv-action:stop-run"), "no run: no stop").toBeNull();
    runStarts(gateway, "reviewer", "run-launch-1");
    await waitFor(() => expect(screen.queryByTestId("conv-action:stop-run")).not.toBeNull(), { timeout: 4_000 });
    await act(async () => { fireEvent.pointerDown(screen.getByTestId("conv-menu"), { button: 0, pointerType: "mouse" }); fireEvent.click(screen.getByTestId("conv-menu")); });
    await waitFor(() => expect(screen.queryByTestId("conv-action:retire")).not.toBeNull(), { timeout: 4_000 });
    expect(screen.queryByTestId("conv-action:respawn")).not.toBeNull();
  }, 20_000);

  it("offers a launch mob's member Stop as before when its row says nothing of stopping", async () => {
    saveChatPane("reviewer", "Launch reviewer");
    const gateway = createTwoRootGateway({ affordances: { launch: { can_retire: true } } });
    await start(gateway, "reviewer");
    runStarts(gateway, "reviewer", "run-launch-2");
    await waitFor(() => expect(screen.queryByTestId("conv-action:stop-run")).not.toBeNull(), { timeout: 4_000 });
  }, 20_000);
});
