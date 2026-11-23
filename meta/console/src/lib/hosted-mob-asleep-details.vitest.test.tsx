import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type RecordedRequest, type TwoRootGateway } from "./fixtures/two-root-gateway";

// An agent's Details pane and a hosted root that is asleep. The pane reads
// the agent and lists its schedules on its own: when it opens, when the
// layout changes, and after every frame that asks the console to refresh.
// None of that is a person's act, so none of it names a root the list says
// is asleep: the root stays asleep with the pane open. A person's "Refresh
// now" is sent, and wakes it.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const MEMBER = "support/reviewer";
const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

/** The layout the browser saved: one pane, the Details of the hosted root's member. */
function saveDetailsPane() {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-details" } }],
    panels: [{
      id: "panel-details",
      target: { id: `inspect:${MEMBER}`, kind: "identity-inspect", identity: MEMBER, memberId: MEMBER, title: "Support reviewer Details", subtitle: "reviewer", iconName: "i-terminal" },
      mode: "console",
    }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-details",
  }));
}

async function start(gateway: TwoRootGateway) {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId("sidebar-mob-state:archive");
  await settle(900);
}

/** The requests that name one mob (`mob_id`). A read that asks for every mob (`mobs`) names none. */
const namingOneMob = (requests: RecordedRequest[]) =>
  requests.filter((request) => "mob_id" in (request.params ?? {}) || request.query.some(([name]) => name === "mob_id"))
    .map((request) => [request.method ?? request.route, request.params?.mob_id]);
/** A frame of the launch mob that makes the console refresh its lists and its open panes. */
const refresh = (gateway: TwoRootGateway) => act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });

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

describe("the Details pane of an agent whose root is unavailable", () => {
  it("says that the team is unavailable, offers no way to wake it, and names the root in no request", async () => {
    saveDetailsPane();
    const gateway = createTwoRootGateway({ hostedState: "unavailable" });
    await start(gateway);
    expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Unavailable");
    expect(await screen.findByTestId(`inspect-panel:${MEMBER}`)).toBeTruthy();
    refresh(gateway);
    await settle(900);
    expect(namingOneMob(gateway.requests)).toEqual([]);
    // Not "asleep", and nothing to press that would name the team.
    const notLoaded = screen.getByTestId(`inspect-asleep:${MEMBER}`);
    expect(notLoaded.textContent).toContain("Support team is unavailable");
    expect(notLoaded.textContent).not.toContain("asleep");
    expect(notLoaded.textContent).not.toContain("Wake");
    expect(notLoaded.querySelector("button")).toBeNull();
    const schedules = screen.getByTestId(`schedules-not-loaded:${MEMBER}`).textContent || "";
    expect(schedules).toBe("This agent's team is unavailable, so its schedules were not read.");
  }, 20_000);
});

describe("the Details pane of an agent whose root is asleep", () => {
  it("names the root in no request when the browser restores it", async () => {
    saveDetailsPane();
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep");
    // The pane is back, and the console refreshes on its own once more.
    expect(await screen.findByTestId(`inspect-panel:${MEMBER}`)).toBeTruthy();
    refresh(gateway);
    await settle(900);
    expect(namingOneMob(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(0);
    expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep");
    // The pane says why it shows nothing, by the team's name, and how to read.
    const asleep = screen.getByTestId(`inspect-asleep:${MEMBER}`);
    expect(asleep.textContent).toContain("Support team is asleep");
    expect(asleep.textContent).toContain("Wake and read details");
    expect(screen.getByTestId(`schedules-not-loaded:${MEMBER}`).textContent).toContain("asleep");
    expect(screen.queryByTestId(`schedules-empty:${MEMBER}`)).toBeNull();
    expect(screen.queryByTestId(`schedules-load-error:${MEMBER}`)).toBeNull();
  }, 20_000);

  it("is read when the person presses the pane's own offer: the root wakes once", async () => {
    saveDetailsPane();
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    const asleep = await screen.findByTestId(`inspect-asleep:${MEMBER}`);
    gateway.clear();
    await act(async () => { fireEvent.click(asleep.querySelector("button")!); });
    await waitFor(() => expect(namingOneMob(gateway.requests)).toContainEqual(["meta/console/inspect_identity", HOSTED_MOB]));
    expect(gateway.wakes()).toBe(1);
  }, 20_000);

  it("names the root in no request after the root went to sleep with the pane open", async () => {
    saveDetailsPane();
    const gateway = createTwoRootGateway();
    await start(gateway);
    expect(await screen.findByTestId(`inspect-panel:${MEMBER}`)).toBeTruthy();
    // While the root is loaded the pane reads through its door.
    await waitFor(() => expect(namingOneMob(gateway.requests).some(([method]) => method === "meta/console/inspect_identity")).toBe(true));

    gateway.set({ hostedState: "dormant" });
    refresh(gateway);
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    await settle(600);
    gateway.clear();
    const wakesBefore = gateway.wakes();

    // Frames of other mobs keep asking the console to refresh.
    refresh(gateway);
    await settle(700);
    refresh(gateway);
    await settle(900);
    expect(gateway.requests.length).toBeGreaterThan(0);
    expect(namingOneMob(gateway.requests)).toEqual([]);
    expect(gateway.wakes()).toBe(wakesBefore);
    expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep");
  }, 20_000);

  it("shows the root as open once the person woke it from the pane: the console reads its lists again", async () => {
    saveDetailsPane();
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    const asleep = await screen.findByTestId(`inspect-asleep:${MEMBER}`);
    expect(screen.getByTestId(`schedules-not-loaded:${MEMBER}`)).toBeTruthy();
    await act(async () => { fireEvent.click(asleep.querySelector("button")!); });
    // Without waiting for the console's periodic refresh: the roster lists the root's agents again,
    // and nothing in the pane still says that the team is asleep.
    expect(await screen.findByTestId("sidebar-agent:support/triage", undefined, { timeout: 4_000 })).toBeTruthy();
    await waitFor(() => expect(screen.queryByTestId(`schedules-not-loaded:${MEMBER}`)).toBeNull(), { timeout: 4_000 });
    expect(screen.queryByTestId(`inspect-asleep:${MEMBER}`)).toBeNull();
  }, 20_000);

  it("is read again when the person asks: that request names the root and wakes it", async () => {
    saveDetailsPane();
    const gateway = createTwoRootGateway({ hostedState: "dormant" });
    await start(gateway);
    await screen.findByTestId(`inspect-panel:${MEMBER}`);
    await settle(600);
    gateway.clear();
    await act(async () => { fireEvent.click(await screen.findByTestId(`inspect-menu:${MEMBER}`)); });
    await act(async () => { fireEvent.click(await screen.findByTestId("inspect-menu:refresh")); });
    await waitFor(() => expect(namingOneMob(gateway.requests)).toContainEqual(["meta/console/inspect_identity", HOSTED_MOB]));
    expect(gateway.wakes()).toBe(1);
  }, 20_000);
});
