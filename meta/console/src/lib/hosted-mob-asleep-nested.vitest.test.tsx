import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type RecordedRequest, type TwoRootGateway } from "./fixtures/two-root-gateway";

// A mob a member created under a hosted root is served through the root's
// door, and is listed only while the root is loaded. When the root sleeps
// the console still knows whose door served that mob: a chat with one of
// its members stays, nothing the console does on its own asks for the
// member (by the root's door or by any other), and the person's message
// goes through the root's door and wakes the root. The same after the page
// is loaded again.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const MEMBER = "support/support-lab/tester";
/** The member as the hosted root's door knows it, on the wire. */
const AT_DOOR = "support-lab/tester";
const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

function saveChat() {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
    panels: [{ id: "panel-chat", target: { id: MEMBER, kind: "agent-chat", addressingMode: "identity", memberId: MEMBER, identity: MEMBER, title: "Tester" }, mode: "console" }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-chat",
  }));
}

/** The requests about the nested mob's member, by any door: `[mob_id, identity]`. */
const aboutTheMember = (requests: RecordedRequest[]) => requests
  .filter((request) => request.params?.identity === AT_DOOR || request.query.some(([name, value]) => name === "identity" && value === AT_DOOR))
  .map((request) => [request.method ?? request.route, request.params?.mob_id ?? null]);

async function leftToItself(gateway: TwoRootGateway) {
  act(() => {
    gateway.push({ identity: "planner", kind: "state_changed", payload: {} });
    gateway.push({ mob: HOSTED_MOB, identity: AT_DOOR, kind: "interaction_complete", interactionId: "while-asleep", payload: {} });
  });
  await settle(500);
  act(() => { gateway.gap(); });
  await settle(700);
}

async function write(text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${MEMBER}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
  await settle();
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

describe("a chat with a member of a mob nested under a hosted root", () => {
  it("stays when the root goes to sleep, asks nothing on its own, and writes through the root's door", async () => {
    saveChat();
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId(`chat-composer:${MEMBER}`);
    await settle(900);

    gateway.set({ hostedState: "dormant" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    await settle(500);
    gateway.clear();
    await leftToItself(gateway);
    // The chat is still there; nothing asked for its member, through any door.
    expect(screen.getByTestId(`chat-composer:${MEMBER}`)).toBeTruthy();
    expect(aboutTheMember(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(0);

    await write("wake up");
    expect(aboutTheMember(gateway.calls("meta/console/send"))).toEqual([["meta/console/send", HOSTED_MOB]]);
  }, 20_000);

  it("is there again after the page is loaded with the root asleep, and asks nothing on its own", async () => {
    saveChat();
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId(`chat-composer:${MEMBER}`);
    await settle(900);
    // The page is loaded again while the root sleeps.
    cleanup();
    gateway.set({ hostedState: "dormant" });
    gateway.clear();
    render(<ConsoleApp baseUrl="" />);
    expect(await screen.findByTestId(`chat-composer:${MEMBER}`)).toBeTruthy();
    await settle(900);
    await leftToItself(gateway);
    expect(screen.getByTestId(`chat-composer:${MEMBER}`)).toBeTruthy();
    expect(aboutTheMember(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(0);
  }, 20_000);
});
