import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createConsoleSendAttempt } from "../../../packages/console-core/src/send-attempt";
import { consoleSendStorageKey, saveConsoleSendAttempts } from "./send-attempt-storage";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type RecordedRequest, type TwoRootGateway } from "./fixtures/two-root-gateway";

// The whole console and a hosted root that is asleep. The console keeps
// lazy loading true: once its list says a root is asleep, nothing the
// console does on its own (its refresh, its repair after a gap in the
// stream, its refresh after a frame) names that root. A person's act does:
// opening the root in the roster, or writing in a chat that is still open.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

async function open(identity: string) {
  await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${identity}`)); });
  return await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
}

async function write(identity: string, text: string) {
  const textarea = await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
  await act(async () => {
    fireEvent.change(textarea, { target: { value: text } });
    fireEvent.keyDown(textarea, { key: "Enter" });
    await Promise.resolve();
  });
  await settle();
}

async function start(gateway: TwoRootGateway) {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId("chat-composer:planner");
  await settle(500);
}

/** The requests that name one mob (`mob_id`). A read that asks for every mob (`mobs`) names none. */
const namingOneMob = (requests: RecordedRequest[]) =>
  requests.filter((request) => "mob_id" in (request.params ?? {}) || request.query.some(([name]) => name === "mob_id"));

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

describe("a console whose list says a hosted root is asleep", () => {
  it("names that root in no request it makes on its own, and wakes it when the person writes", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await open("support/reviewer");
    await settle();

    // The root goes to sleep; a frame of the launch mob makes the console read its lists again.
    gateway.set({ hostedState: "dormant" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    expect(screen.queryByTestId("sidebar-agent:support/reviewer")).toBeNull();
    gateway.clear();

    // From here the console acts on its own: a frame of the sleeping root's
    // member ends an interaction, the stream reports a gap it cannot replay,
    // and another frame asks for a refresh of the lists.
    act(() => {
      gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_complete", interactionId: "while-asleep", payload: {} });
      gateway.gap();
    });
    await settle(800);
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await settle(1_200);
    expect(gateway.requests.length).toBeGreaterThan(0);
    expect(namingOneMob(gateway.requests)).toEqual([]);

    // The chat that was open is still there, and writing in it wakes the root.
    await write("support/reviewer", "wake up");
    const sent = gateway.calls("meta/console/send");
    expect(sent.map((request) => [request.params?.mob_id, request.params?.identity, request.params?.content]))
      .toEqual([[HOSTED_MOB, "reviewer", "wake up"]]);
  });

  it("opens the root when the person opens it in the roster: one request names it, then its members are listed", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    expect((await screen.findByTestId("sidebar-mob-state:support")).textContent).toBe("Asleep");
    expect(namingOneMob(gateway.requests)).toEqual([]);
    gateway.clear();

    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support")); });
    expect(await screen.findByTestId("sidebar-agent:support/reviewer")).toBeTruthy();
    expect(namingOneMob(gateway.requests).map((request) => [request.method, request.params?.mob_id])).toEqual([["meta/status", HOSTED_MOB]]);
  });

  it("sends one opening request when the person clicks the root again while it is opening", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    await screen.findByTestId("sidebar-mob-state:support");
    gateway.clear();
    // Three clicks before the first answer is in.
    await act(async () => {
      fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support"));
      fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support"));
      fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support"));
    });
    expect(await screen.findByTestId("sidebar-agent:support/reviewer")).toBeTruthy();
    expect(namingOneMob(gateway.requests).map((request) => [request.method, request.params?.mob_id])).toEqual([["meta/status", HOSTED_MOB]]);
  });

  it("does not say the root could not be opened when it was opened and only the read of the list after it failed", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    await screen.findByTestId("sidebar-mob-state:support");
    gateway.clear();
    gateway.set({ experienceFailures: 1 });
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support")); });
    await waitFor(() => expect(gateway.calls("meta/status").length).toBe(1));
    await waitFor(() => expect(gateway.requests.some((request) => request.route === "GET /console/experience")).toBe(true));
    await settle(400);
    expect(gateway.wakes()).toBe(1);
    expect(screen.queryByTestId("console-action-error")?.textContent || "").not.toContain("Could not open");
    // The root is no longer shown as waking; the console's next read of its lists shows its members.
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    expect(await screen.findByTestId("sidebar-agent:support/reviewer", undefined, { timeout: 4_000 })).toBeTruthy();
  }, 20_000);

  it("does not read for a chat the browser restored while its root is asleep, and reads once the root is loaded", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await open("support/reviewer");
    await settle(600);
    // The page is loaded again: the browser restores the open chat.
    cleanup();
    gateway.set({ hostedState: "dormant" });
    gateway.clear();
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:support/reviewer");
    await settle(1_200);
    expect((await screen.findByTestId("sidebar-mob-state:support")).textContent).toBe("Asleep");
    expect(namingOneMob(gateway.requests)).toEqual([]);

    // Someone else wakes the root; the console's next refresh lists it as loaded.
    gateway.set({ hostedState: "running" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(
      namingOneMob(gateway.requests).map((request) => [request.method, request.params?.mob_id, request.params?.identity]),
    ).toEqual([["meta/console/query_timeline", HOSTED_MOB, "reviewer"]]), { timeout: 4_000 });
  }, 20_000);

  it("does not send a queued message on its own while the root is asleep: it stays queued until a person acts", async () => {
    // The browser kept a chat with the root's member open, and a message the person queued for it.
    const namespace = JSON.stringify(["", "console-storage:v1:two-root"]);
    window.localStorage.setItem(`meta-console-dock-state:${namespace}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
      panels: [{
        id: "panel-chat",
        target: { id: "support/reviewer", kind: "agent-chat", addressingMode: "identity", memberId: "support/reviewer", identity: "support/reviewer", title: "Support reviewer", iconName: "i-team" },
        mode: "console",
      }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-chat",
    }));
    saveConsoleSendAttempts(window.localStorage, namespace, "support/reviewer", [createConsoleSendAttempt({
      id: "queued-1", scope: namespace, destination: "support/reviewer", origin: "console:pane", idempotencyKey: "key-queued-1", text: "Queued before the reload", now: 1,
    })]);
    const queued = () => window.localStorage.getItem(consoleSendStorageKey(namespace, "support/reviewer")) || "";

    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:support/reviewer");
    await settle(1_500);
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await settle(900);
    // Everything else that makes the console try its queues again: the
    // window gets the focus, the network is back, a frame says the member
    // finished something.
    act(() => {
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("online"));
      gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_complete", interactionId: "earlier", payload: {} });
    });
    await settle(900);
    expect(gateway.calls("meta/console/send")).toEqual([]);
    expect(namingOneMob(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(0);
    expect(queued()).toContain("Queued before the reload");
    // The message is shown as waiting, with the reason, by the team's name.
    const waiting = screen.getByTestId("pending-waiting:support/reviewer");
    expect(waiting.textContent).toBe("Waiting: Support team is asleep. Queued messages are sent when the team is open.");

    // The person opens the team in the roster: the message is sent.
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support")); });
    await waitFor(() => expect(
      gateway.calls("meta/console/send").map((request) => [request.params?.mob_id, request.params?.identity, request.params?.content]),
    ).toEqual([[HOSTED_MOB, "reviewer", "Queued before the reload"]]), { timeout: 6_000 });
    await waitFor(() => expect(screen.queryByTestId("pending-waiting:support/reviewer")).toBeNull());
  }, 30_000);

  it("holds a message queued while the member was busy once the root has gone to sleep, and sends it when the person opens the root", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await open("support/reviewer");
    await settle();
    // The member is at work: what the person writes now waits in the queue.
    act(() => { gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_started", interactionId: "busy-1", payload: { content: "working", prompt: "working" } }); });
    await settle(400);
    await write("support/reviewer", "Queued while the agent was busy");
    expect(gateway.calls("meta/console/send")).toEqual([]);

    // The root goes to sleep with the message still queued.
    gateway.set({ hostedState: "dormant" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    gateway.clear();
    // The member's work ends, the window gets the focus, the network is back:
    // everything that makes the console try its queues again.
    act(() => {
      gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_complete", interactionId: "busy-1", payload: {} });
      window.dispatchEvent(new Event("focus"));
      window.dispatchEvent(new Event("online"));
    });
    await settle(1_200);
    expect(gateway.calls("meta/console/send")).toEqual([]);
    expect(namingOneMob(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(0);
    expect(screen.getByTestId("pending-waiting:support/reviewer").textContent)
      .toBe("Waiting: Support team is asleep. Queued messages are sent when the team is open.");

    // The person opens the team in the roster: the message is sent.
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support")); });
    await waitFor(() => expect(
      gateway.calls("meta/console/send").map((request) => [request.params?.mob_id, request.params?.identity, request.params?.content]),
    ).toEqual([[HOSTED_MOB, "reviewer", "Queued while the agent was busy"]]), { timeout: 6_000 });
  }, 30_000);

  it("shows in an interactive view's place that such views are not available for an agent of another team, loaded or asleep, with nothing to press", async () => {
    const gateway = createTwoRootGateway({
      experience: { console_config: { title: "Operations", layout: { initial_agent: "planner" }, mcp_apps_sandbox_url: "https://sandbox.test/sandbox.html" } },
    });
    await start(gateway);
    // A tool of the launch mob's agent answered with an interactive view: it is drawn as before.
    act(() => {
      gateway.push({
        identity: "planner", kind: "mcp_app",
        payload: { session_id: "session:launch", tool_call_id: "call:launch", fallback: "A table of the day" },
        extra: { source: { kind: "tool_application" }, session_id: "session:launch" },
      });
    });
    await waitFor(() => expect(document.querySelector("[data-mcp-app-call='call:launch']")).not.toBeNull(), { timeout: 4_000 });

    await open("support/reviewer");
    await settle();
    // A tool of the hosted root's agent answered with one too.
    act(() => {
      gateway.push({
        mob: HOSTED_MOB, identity: "reviewer", kind: "mcp_app",
        payload: { session_id: "session:app", tool_call_id: "call:app", fallback: "A chart of the week" },
        extra: { source: { kind: "tool_application" }, session_id: "session:app" },
      });
    });
    const sentence = "Interactive views are not available for Support reviewer yet. For now they work with agents of Ops team only.";
    // While the root is loaded: the limit, the tool's own words, no view and nothing to press.
    const place = await screen.findByTestId("mcp-app-unavailable:call:app", undefined, { timeout: 4_000 });
    expect(place.textContent).toContain(sentence);
    expect(place.textContent).toContain("A chart of the week");
    expect(place.querySelector("button")).toBeNull();
    expect(document.querySelector("[data-mcp-app-call='call:app']")).toBeNull();

    // The root goes to sleep: the same place, the same words; waking the team would not load the view.
    gateway.set({ hostedState: "dormant" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    gateway.clear();
    await settle(700);
    const asleep = screen.getByTestId("mcp-app-unavailable:call:app");
    expect(asleep.textContent).toContain(sentence);
    expect(asleep.textContent).not.toContain("asleep");
    expect(asleep.querySelector("button")).toBeNull();
    expect(screen.queryByTestId("mcp-app-asleep:call:app")).toBeNull();
    expect(namingOneMob(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(0);
  }, 30_000);

  it("says of a queued message that its team is unavailable, where it is: not that it is asleep", async () => {
    const namespace = JSON.stringify(["", "console-storage:v1:two-root"]);
    window.localStorage.setItem(`meta-console-dock-state:${namespace}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
      panels: [{
        id: "panel-chat",
        target: { id: "support/reviewer", kind: "agent-chat", addressingMode: "identity", memberId: "support/reviewer", identity: "support/reviewer", title: "Support reviewer", iconName: "i-team" },
        mode: "console",
      }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-chat",
    }));
    saveConsoleSendAttempts(window.localStorage, namespace, "support/reviewer", [createConsoleSendAttempt({
      id: "queued-1", scope: namespace, destination: "support/reviewer", origin: "console:pane", idempotencyKey: "key-queued-1", text: "Queued before the reload", now: 1,
    })]);
    const gateway = createTwoRootGateway({ hostedState: "unavailable" });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:support/reviewer");
    await settle(1_500);
    expect(gateway.calls("meta/console/send")).toEqual([]);
    expect(namingOneMob(gateway.requests)).toEqual([]);
    expect(screen.getByTestId("pending-waiting:support/reviewer").textContent)
      .toBe("Waiting: Support team is unavailable. Queued messages are sent when the team is available again.");
  }, 20_000);

  it("says what the newest list says of a team that is not loaded, also before the page has taken the roster read with it", async () => {
    const namespace = JSON.stringify(["", "console-storage:v1:two-root"]);
    window.localStorage.setItem(`meta-console-dock-state:${namespace}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
      panels: [{
        id: "panel-chat",
        target: { id: "support/reviewer", kind: "agent-chat", addressingMode: "identity", memberId: "support/reviewer", identity: "support/reviewer", title: "Support reviewer", iconName: "i-team" },
        mode: "console",
      }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-chat",
    }));
    saveConsoleSendAttempts(window.localStorage, namespace, "support/reviewer", [createConsoleSendAttempt({
      id: "queued-1", scope: namespace, destination: "support/reviewer", origin: "console:pane", idempotencyKey: "key-queued-1", text: "Queued before the reload", now: 1,
    })]);
    const gateway = createTwoRootGateway({ hostedState: "unavailable" });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:support/reviewer");
    await settle(1_200);
    const note = () => screen.getByTestId("pending-waiting:support/reviewer").textContent;
    expect(note()).toBe("Waiting: Support team is unavailable. Queued messages are sent when the team is available again.");

    // The team is now asleep (a person can wake it). The roster is read
    // again; the connection has the new list at once, the page takes the
    // new roster only when the modules, which answer late, are in too.
    gateway.set({ hostedState: "dormant", modulesDelayMs: 2_000 });
    const reads = () => gateway.requests.filter((request) => request.route === "GET /console/experience").length;
    const before = reads();
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(reads()).toBeGreaterThan(before), { timeout: 3_000 });
    await settle(300);
    act(() => { gateway.push({ identity: "planner", kind: "assistant_text_delta", payload: { text: "a word " } }); });
    await settle(100);
    // The page still shows the older roster, where the team is unavailable.
    expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Unavailable");
    // What is said of the queued message follows the list that holds it back.
    expect(note()).toBe("Waiting: Support team is asleep. Queued messages are sent when the team is open.");
    expect(gateway.calls("meta/console/send")).toEqual([]);

    // The modules are in: the page says the same.
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 5_000 });
    expect(note()).toBe("Waiting: Support team is asleep. Queued messages are sent when the team is open.");
  }, 30_000);

  it("sends the same queued message on its own when the root is loaded, as it does for any agent", async () => {
    const namespace = JSON.stringify(["", "console-storage:v1:two-root"]);
    window.localStorage.setItem(`meta-console-dock-state:${namespace}`, JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-chat" } }],
      panels: [{
        id: "panel-chat",
        target: { id: "support/reviewer", kind: "agent-chat", addressingMode: "identity", memberId: "support/reviewer", identity: "support/reviewer", title: "Support reviewer", iconName: "i-team" },
        mode: "console",
      }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-chat",
    }));
    saveConsoleSendAttempts(window.localStorage, namespace, "support/reviewer", [createConsoleSendAttempt({
      id: "queued-1", scope: namespace, destination: "support/reviewer", origin: "console:pane", idempotencyKey: "key-queued-1", text: "Queued before the reload", now: 1,
    })]);
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("chat-composer:support/reviewer");
    await waitFor(() => expect(
      gateway.calls("meta/console/send").map((request) => [request.params?.mob_id, request.params?.identity, request.params?.content]),
    ).toEqual([[HOSTED_MOB, "reviewer", "Queued before the reload"]]), { timeout: 6_000 });
  }, 20_000);

  it("says that the gateway refused, by the root's name and never by its id, when the opening of the root is refused", async () => {
    const gateway = createTwoRootGateway({ hostedState: "dormant", refuseOpen: true });
    await start(gateway);
    await screen.findByTestId("sidebar-mob-state:support");
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support")); });
    const said = (await screen.findByTestId("console-action-error")).textContent || "";
    // A refusal is an answer: it is not said as a failure to try again.
    expect(said).toContain("The gateway refused to open Support team. It was not opened.");
    expect(said).not.toContain("Try again");
    expect(said.toLowerCase().includes("support ")).toBe(true);
    expect(/\bsupport\b(?! team)/.test(said)).toBe(false);
    // The root is still listed as asleep, and no longer as waking.
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"));
  });
});
