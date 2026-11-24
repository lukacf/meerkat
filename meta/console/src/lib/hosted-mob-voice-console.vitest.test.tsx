import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, type TwoRootGateway } from "./fixtures/two-root-gateway";

// Voice on a gateway that hosts a second root. A KNOWN LIMIT of this
// release: voice works with the agents of the mob the gateway was launched
// with. For an agent of another root the console shows its voice control
// as unavailable and says why; it does not hide it, and it asks the
// gateway nothing. For an agent of the launch mob nothing changes.

const voice = vi.hoisted(() => ({
  start: vi.fn(async () => {}),
  close: vi.fn(async () => {}),
}));
vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

async function open(identity: string) {
  await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${identity}`)); });
  await screen.findByTestId(`chat-composer:${identity}`);
  await settle(500);
}

async function start(gateway: TwoRootGateway) {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId("chat-composer:planner");
  await settle(500);
}

const readiness = (gateway: TwoRootGateway) => gateway.calls("meta/console/voice/readiness").map((request) => request.params);
/** What a person reads or hears of a control: its label and its tooltip. */
const said = (element: HTMLElement) => [element.getAttribute("aria-label") || "", element.getAttribute("title") || ""];

let restoreAbortSignal = () => {};
beforeEach(() => {
  restoreAbortSignal = ensureAbortSignalAny();
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
  voice.start.mockClear();
  let pending = Promise.resolve();
  Object.defineProperty(navigator, "locks", { configurable: true, value: { request: (_key: string, callback: () => unknown) => {
    const next = pending.then(callback); pending = next.then(() => {}, () => {}); return next;
  } } });
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.unstubAllGlobals(); restoreAbortSignal();
});

describe("voice for an agent of the launch mob", () => {
  it("is offered exactly as before, on a gateway that hosts a second root", async () => {
    const gateway = createTwoRootGateway({ voice: true });
    await start(gateway);
    await open("reviewer");
    const button = await screen.findByTestId("voice-start");
    await waitFor(() => expect(button.getAttribute("data-readiness")).toBe("confirmed"));
    expect(said(button)).toEqual(["Start voice with Launch reviewer", "Start voice with Launch reviewer"]);
    expect((button as HTMLButtonElement).disabled).toBe(false);
    expect(button.getAttribute("aria-pressed")).toBe("false");
    expect(screen.queryByTestId("voice-unavailable")).toBeNull();
    // Readiness was asked as before: the member alone, no mob.
    expect(readiness(gateway).length).toBeGreaterThan(0);
    expect(readiness(gateway).every((params) => JSON.stringify(Object.keys(params ?? {})) === JSON.stringify(["identity"]))).toBe(true);
    expect(readiness(gateway).some((params) => params?.identity === "reviewer")).toBe(true);
    // And voice starts with the member as before.
    await act(async () => { fireEvent.click(button); });
    expect(voice.start.mock.calls).toEqual([[{ identity: "reviewer", label: "Launch reviewer" }]]);
  });

  it("is offered exactly as before on a gateway that hosts one mob", async () => {
    const gateway = createTwoRootGateway({ voice: true, addressing: false });
    await start(gateway);
    await open("reviewer");
    const button = await screen.findByTestId("voice-start");
    await waitFor(() => expect(button.getAttribute("data-readiness")).toBe("confirmed"));
    expect(said(button)).toEqual(["Start voice with Launch reviewer", "Start voice with Launch reviewer"]);
    expect(screen.queryByTestId("voice-unavailable")).toBeNull();
  });
});

describe("voice for an agent of a root the host created beside the launch mob", () => {
  it("is shown as unavailable, with the reason, and nothing is asked of the gateway", async () => {
    const gateway = createTwoRootGateway({ voice: true });
    await start(gateway);
    await open("support/reviewer");
    const control = await screen.findByTestId("voice-unavailable");
    expect(screen.queryByTestId("voice-start")).toBeNull();
    const reason = "Voice is not available for Support reviewer yet. For now it works with agents of Ops team only.";
    expect(said(control)).toEqual([reason, reason]);
    expect(control.getAttribute("aria-disabled")).toBe("true");
    // No readiness was asked for the hosted root's agent, by any name.
    expect(readiness(gateway).filter((params) => params?.identity !== "planner")).toEqual([]);
    // Pressing it starts nothing and says the reason where the console says what it could not do.
    await act(async () => { fireEvent.click(control); });
    expect(voice.start).not.toHaveBeenCalled();
    expect((await screen.findByTestId("console-action-error")).textContent).toContain(reason);
    // No mob id in what a person reads.
    expect(reason.includes("support")).toBe(false);
  });

  it("is not shown where the console offers no voice at all", async () => {
    const gateway = createTwoRootGateway({ voice: false });
    await start(gateway);
    await open("support/reviewer");
    expect(screen.queryByTestId("voice-unavailable")).toBeNull();
    expect(screen.queryByTestId("voice-start")).toBeNull();
  });
});
