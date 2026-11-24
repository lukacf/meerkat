import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { beginConsoleSendAttempt, createConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB } from "./fixtures/two-root-gateway";
import { saveConsoleSendAttempts } from "./send-attempt-storage";

// "Check" on a message whose delivery is not confirmed asks the member's
// door who the member is and then reads that member's timeline. Two
// members share the name `reviewer`, one in each root: the check of a
// message to the hosted root's reviewer reads the hosted root's reviewer,
// never the launch mob's.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const MEMBER = "support/reviewer";
const NAMESPACE = JSON.stringify(["", "console-storage:v1:two-root"]);
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

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

describe("the check of a message to a hosted root's member", () => {
  it("reads that member's timeline by its door and finds the message there", async () => {
    // The browser kept the chat open and a message whose outcome it does not know.
    window.localStorage.setItem(`meta-console-dock-state:${NAMESPACE}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
      panels: [{ id: "panel-chat", target: { id: MEMBER, kind: "agent-chat", addressingMode: "identity", memberId: MEMBER, identity: MEMBER, title: "Support reviewer" }, mode: "console" }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-chat",
    }));
    const attempted = beginConsoleSendAttempt(createConsoleSendAttempt({
      id: "unconfirmed", scope: NAMESPACE, destination: MEMBER, origin: "console:pane", idempotencyKey: "key-unconfirmed", text: "Did this arrive?", now: 1,
    }), { owner: "another-tab", now: 2, handlingMode: "queue" });
    saveConsoleSendAttempts(window.localStorage, NAMESPACE, MEMBER, [{ ...attempted, state: "outcome-unknown", lease: undefined }]);

    const gateway = createTwoRootGateway();
    // The hosted root's reviewer received it; the launch mob's reviewer did not.
    gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "user_input", interactionId: "delivered", payload: JSON.parse(attempted.envelopeJson!) as Record<string, unknown> });
    gateway.push({ identity: "reviewer", kind: "user_input", interactionId: "other", payload: { content: "something else", idempotency_key: "another-request" } });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId(`chat-composer:${MEMBER}`);
    await settle(900);
    gateway.clear();

    await act(async () => { fireEvent.click(await screen.findByText("Check")); });
    await waitFor(() => expect(gateway.calls("meta/console/query_timeline").length).toBeGreaterThan(0), { timeout: 4_000 });
    await settle(300);
    const reads = gateway.calls("meta/console/query_timeline").filter((request) => request.params?.identity !== undefined)
      .map((request) => [request.params?.mob_id, request.params?.identity]);
    expect(reads).toEqual([[HOSTED_MOB, "reviewer"]]);
    // The message was found: it no longer waits for a check.
    await waitFor(() => expect(screen.queryByText("Check")).toBeNull(), { timeout: 4_000 });
  }, 20_000);
});
