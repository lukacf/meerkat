import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, type TwoRootGateway } from "./fixtures/two-root-gateway";

// What the console decides from its roster (the member of a restored pane,
// a kept mob choice, the grouping it starts with) waits for a roster that
// lists every mob the caller may address. These rows pin the two ways that
// knowledge can fail to arrive: the first read of the gateway's
// capabilities fails, and the roster of every mob carries no list of mobs
// at all (a gateway leaves an empty list out).

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const MEMBER = "support/reviewer";
const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });
const refresh = (gateway: TwoRootGateway) => act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });

function saveChat(identity: string, title: string) {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
    panels: [{ id: "panel-chat", target: { id: identity, kind: "agent-chat", addressingMode: "identity", memberId: identity, identity, title }, mode: "console" }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-chat",
  }));
}

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

describe("a console whose first read of the gateway's capabilities fails", () => {
  it("keeps a restored chat with a hosted root's member and the mob a person chose, and has them once a read answers", async () => {
    // A first visit: the person chooses to see the hosted root alone and opens its member.
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:planner");
    await settle(500);
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-menu:support")); });
    await act(async () => { fireEvent.click(await screen.findByTestId("sidebar-mob-only:support")); });
    await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${MEMBER}`)); });
    await screen.findByTestId(`chat-composer:${MEMBER}`);
    await settle(600);

    // The page is loaded again, and the first read of the capabilities fails.
    cleanup();
    gateway.set({ capabilitiesFailures: 1 });
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("sidebar-agent:planner");
    await settle(900);
    // Nothing was decided from the roster of the launch mob alone.
    expect(screen.queryByTestId("chat-composer:planner")).toBeNull();

    // The console reads the capabilities again with its next refresh.
    refresh(gateway);
    expect(await screen.findByTestId(`chat-composer:${MEMBER}`, undefined, { timeout: 5_000 })).toBeTruthy();
    await waitFor(() => expect(screen.queryByTestId("sidebar-agent:planner")).toBeNull(), { timeout: 4_000 });
    expect(screen.getByTestId(`sidebar-agent:${MEMBER}`)).toBeTruthy();
  }, 30_000);
});

describe("a console whose reads of the gateway's capabilities never answer", () => {
  it("asks a few more times with its roster reads, then stops, and from then on holds its roster to be whole", async () => {
    // The mob the browser keeps as the person's choice, as it is stored.
    const keptChoice = () => Array.from({ length: window.localStorage.length }, (_, index) => window.localStorage.key(index) || "")
      .filter((key) => key.startsWith("meta-console-mob-view"))
      .map((key) => { try { return (JSON.parse(window.localStorage.getItem(key) || "null") as { mob_id?: unknown } | null)?.mob_id; } catch { return null; } })
      .filter((value): value is string => typeof value === "string" && value !== "");
    // A first visit on a gateway that answers: the person chooses to see the hosted root alone.
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:planner");
    await settle(500);
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-menu:support")); });
    await act(async () => { fireEvent.click(await screen.findByTestId("sidebar-mob-only:support")); });
    await settle(400);
    expect(keptChoice()).toEqual(["support"]);

    // The page is loaded again, and from now on the gateway's capability read never answers.
    cleanup();
    gateway.set({ capabilitiesFailures: 1_000 });
    gateway.clear();
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("sidebar-agent:planner");
    await settle(900);
    const first = gateway.calls("meta/capabilities").length;
    expect(first).toBeGreaterThan(0);
    // While the console still asks, its roster may not be the whole one: the kept choice is not judged by it.
    expect(keptChoice()).toEqual(["support"]);

    // Many reads of the roster follow (each frame asks for one).
    for (let turn = 0; turn < 8; turn += 1) {
      refresh(gateway);
      await settle(350);
    }
    const rosterReads = gateway.requests.filter((request) => request.route === "GET /console/experience").length;
    expect(rosterReads).toBeGreaterThan(6);
    // Not one more capability read with every roster read: a small number, then none.
    expect(gateway.calls("meta/capabilities").length - first).toBeLessThanOrEqual(3);

    // Having stopped, the console holds its roster to be whole, as with a
    // gateway that hosts one mob: the mob the person chose is not in it, so
    // the choice is given up.
    await waitFor(() => expect(keptChoice()).toEqual([]), { timeout: 5_000 });
    const stopped = gateway.calls("meta/capabilities").length;
    refresh(gateway);
    await settle(500);
    expect(gateway.calls("meta/capabilities").length).toBe(stopped);
  }, 30_000);
});

describe("a console whose roster of every mob comes to carry no list of mobs", () => {
  const keys = (storage: Storage) => Array.from({ length: storage.length }, (_, index) => storage.key(index) || "");
  const keysOfSupport = () => [...keys(window.localStorage), ...keys(window.sessionStorage)].filter((key) => key.includes("support%2F"));

  it("holds a whole roster all the same: the open chat of a member that is gone is replaced, and nothing of the root stays in the browser", async () => {
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:planner");
    await settle(500);
    await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${MEMBER}`)); });
    const composer = await screen.findByTestId(`chat-composer:${MEMBER}`) as HTMLTextAreaElement;
    await act(async () => { fireEvent.change(composer, { target: { value: "a draft for the hosted reviewer" } }); });
    await settle(600);
    expect(keysOfSupport().length).toBeGreaterThan(0);

    // The caller may address no mob beside the launch mob's own any more:
    // the gateway leaves the empty list out of its answer.
    gateway.set({ listNoMobs: true });
    refresh(gateway);
    await waitFor(() => expect(screen.queryByTestId(`chat-composer:${MEMBER}`)).toBeNull(), { timeout: 5_000 });
    await waitFor(() => expect(keysOfSupport()).toEqual([]), { timeout: 5_000 });
    // The console goes on with a member that is listed.
    expect(await screen.findByTestId("sidebar-agent:planner")).toBeTruthy();
  }, 30_000);
});

