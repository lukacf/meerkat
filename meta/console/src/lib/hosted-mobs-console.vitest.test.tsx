import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type RecordedRequest, type TwoRootGateway } from "./fixtures/two-root-gateway";

// The whole console on a gateway that hosts a second root: the roster
// lists the members of both roots, a chat with a member of the hosted root
// writes through that root's door, and a frame of the hosted root shows in
// its own member's chat. Two members share the name `reviewer`, one in
// each root.

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

const last = (requests: RecordedRequest[]) => requests[requests.length - 1];
const streams = (gateway: TwoRootGateway) => gateway.requests.filter((request) => request.route === "GET /console/timeline/stream");

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

describe("a console on a gateway that hosts a second root", () => {
  it("lists the members of both roots soon after it opens, without waiting for its periodic refresh", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    for (const identity of ["planner", "reviewer", "research/scout", "support/reviewer", "support/triage"]) {
      expect(await screen.findByTestId(`sidebar-agent:${identity}`)).toBeTruthy();
    }
    // The roster draws only the rows in view: the last mob's one member is
    // read from its header's count.
    expect((await screen.findByTestId("sidebar-mob-count:support-lab")).textContent).toBe("1");
    // Its one stream asks for every mob the caller may address.
    expect(last(streams(gateway))?.query).toEqual([["mobs", "*"]]);
  });

  it("writes to a member of the hosted root through that root's door", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await open("support/reviewer");
    await write("support/reviewer", "hello hosted reviewer");
    const sent = last(gateway.calls("meta/console/send"));
    expect({ mob_id: sent?.params?.mob_id, identity: sent?.params?.identity, content: sent?.params?.content })
      .toEqual({ mob_id: HOSTED_MOB, identity: "reviewer", content: "hello hosted reviewer" });
    // The chat's own history was read through the same door.
    const read = last(gateway.calls("meta/console/query_timeline").filter((request) => request.params?.identity !== undefined));
    expect({ mob_id: read?.params?.mob_id, identity: read?.params?.identity }).toEqual({ mob_id: HOSTED_MOB, identity: "reviewer" });
  });

  it("keeps an open chat with a member of the hosted root when the page is loaded again", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await open("support/reviewer");
    await settle(600);
    // The page is loaded again: the browser restores the open chat. The
    // console's first roster lists the launch mob alone (the gateway's
    // capabilities answer after it); the chat is back once the roster of
    // every mob lists its member.
    cleanup();
    gateway.set({ capabilitiesDelayMs: 200 });
    gateway.clear();
    render(<ConsoleApp baseUrl="" />);
    expect(await screen.findByTestId("chat-composer:support/reviewer")).toBeTruthy();
    await settle(600);
    expect(screen.getByTestId("chat-composer:support/reviewer")).toBeTruthy();
    expect(screen.queryByTestId("chat-composer:planner")).toBeNull();
    // The first roster was read before the console knew what the gateway hosts.
    const experiences = gateway.requests.filter((request) => request.route === "GET /console/experience");
    expect(experiences[0]?.query).toEqual([]);
    expect(experiences[experiences.length - 1]?.query).toEqual([["mobs", "*"]]);
  });

  it("keeps the mob a person chose to see when the page is loaded again", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-menu:support")); });
    await act(async () => { fireEvent.click(await screen.findByTestId("sidebar-mob-only:support")); });
    await waitFor(() => expect(screen.queryByTestId("sidebar-agent:planner")).toBeNull());
    expect(screen.getByTestId("sidebar-agent:support/reviewer")).toBeTruthy();
    // Loaded again, with a first roster that lists the launch mob alone:
    // the choice names a mob that roster does not hold, and it is kept.
    cleanup();
    gateway.set({ capabilitiesDelayMs: 200 });
    render(<ConsoleApp baseUrl="" />);
    expect(await screen.findByTestId("sidebar-agent:support/reviewer")).toBeTruthy();
    await settle(600);
    expect(screen.queryByTestId("sidebar-agent:planner")).toBeNull();
    expect(screen.getByTestId("sidebar-agent:support/reviewer")).toBeTruthy();
  });

  it("groups the roster by mob from the start when the gateway hosts a second root, though its first roster lists one mob", async () => {
    const gateway = createTwoRootGateway({ launchNested: false, capabilitiesDelayMs: 200 });
    await start(gateway);
    await settle(600);
    expect(await screen.findByTestId("sidebar-mob:support")).toBeTruthy();
    expect(screen.getByTestId("sidebar-mob:ops-team")).toBeTruthy();
    expect(screen.getByTestId("sidebar-agent:support/reviewer")).toBeTruthy();
  });

  it("shows a frame of the hosted root in its own member's chat and not in the launch member's of the same name", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await open("support/reviewer");
    await settle();
    act(() => {
      gateway.push({ mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_started", interactionId: "hosted-1", payload: { content: "asked in the hosted root", prompt: "asked in the hosted root" } });
      gateway.push({ identity: "reviewer", kind: "interaction_started", interactionId: "launch-1", payload: { content: "asked in the launch mob", prompt: "asked in the launch mob" } });
    });
    await waitFor(() => expect(screen.getByTestId("chat-pane:support/reviewer").textContent).toContain("asked in the hosted root"));
    expect(screen.getByTestId("chat-pane:support/reviewer").textContent).not.toContain("asked in the launch mob");
    await open("reviewer");
    await waitFor(() => expect(screen.getByTestId("chat-pane:reviewer").textContent).toContain("asked in the launch mob"));
    expect(screen.getByTestId("chat-pane:reviewer").textContent).not.toContain("asked in the hosted root");
  });

  it("shows nothing of a frame whose mob is in no list it holds, and asks nothing by that mob's id", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await settle(600);
    gateway.clear();
    act(() => {
      gateway.push({ mob: "newcomer", identity: "reviewer", kind: "interaction_started", interactionId: "n-1", payload: { content: "asked in a mob not listed", prompt: "asked in a mob not listed" } });
      gateway.push({ mob: "newcomer", identity: "reviewer", kind: "interaction_complete", interactionId: "n-1", payload: {} });
      gateway.push({ identity: "planner", kind: "state_changed", payload: {} });
    });
    await waitFor(() => expect(gateway.requests.some((request) => request.route === "GET /console/experience")).toBe(true), { timeout: 4_000 });
    await settle(1_200);
    expect(gateway.requests.filter((request) => JSON.stringify([request.params ?? {}, request.query]).includes("newcomer"))).toEqual([]);
    expect(document.body.textContent || "").not.toContain("newcomer");
    expect(document.body.innerHTML).not.toContain("newcomer");
  }, 20_000);

  it("shows the connections of a hosted root's agent as agents of that root, not as launch agents of the same name", async () => {
    // The Details of the hosted root's `triage`, which is wired to that root's `reviewer`.
    window.localStorage.setItem('meta-console-dock-state:["","console-storage:v1:two-root"]', JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "inspect:support/triage", kind: "identity-inspect", identity: "support/triage", memberId: "support/triage", title: "Details" } }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
    const gateway = createTwoRootGateway();
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    const connections = await screen.findByTestId("inspect-connections:support/triage", undefined, { timeout: 8_000 });
    await waitFor(() => expect(connections.textContent).toContain("Support reviewer"), { timeout: 5_000 });
    expect(connections.textContent).not.toContain("Launch reviewer");
  }, 20_000);

  it("shows who created a hosted root's agent as an agent of that root, and opens that agent from the line", async () => {
    window.localStorage.setItem('meta-console-dock-state:["","console-storage:v1:two-root"]', JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "inspect:support/triage", kind: "identity-inspect", identity: "support/triage", memberId: "support/triage", title: "Details" } }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
    // The hosted root's `triage` was created by that root's `reviewer`; the launch mob has a `reviewer` too.
    const gateway = createTwoRootGateway({ agentRecords: true });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    const record = await screen.findByTestId("inspect-record:support/triage", undefined, { timeout: 8_000 });
    await waitFor(() => expect(record.textContent).toContain("Support reviewer"), { timeout: 5_000 });
    expect(record.textContent).not.toContain("Launch reviewer");
    const link = Array.from(record.querySelectorAll("button")).find((button) => (button.textContent || "").includes("Support reviewer"));
    expect(link).toBeTruthy();
    await act(async () => { fireEvent.click(link!); });
    expect(await screen.findByTestId("inspect-panel:support/reviewer", undefined, { timeout: 5_000 })).toBeTruthy();
    expect(screen.queryByTestId("inspect-panel:reviewer")).toBeNull();
  }, 20_000);

  it("names the agent a hosted root's schedule sends as by that root's agent, not the launch agent of the same name", async () => {
    window.localStorage.setItem('meta-console-dock-state:["","console-storage:v1:two-root"]', JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "inspect:support/triage", kind: "identity-inspect", identity: "support/triage", memberId: "support/triage", title: "Details" } }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
    const any = { kind: "any" };
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS),
      // As the hosted root's door writes it: its agent `triage`, sent as its agent `reviewer`.
      hostedSchedules: [{
        schedule_id: "sch-1", identity: "triage", name: "Morning sort", content: "Sort what came in overnight.",
        trigger: { type: "calendar", timezone: "UTC", minute: { kind: "values", values: [0] }, hour: { kind: "values", values: [9] }, day_of_month: any, month: any, day_of_week: any },
        phase: "active", revision: 1, sender: "reviewer",
      }],
    });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    const sender = await screen.findByTestId("schedule-sender:sch-1", undefined, { timeout: 8_000 });
    expect(sender.textContent).toBe("Sends as Support reviewer");
  }, 20_000);

  it("lists a hosted root's schedule in the Schedules panel under that root's agent, with its sender as that root's agent, and changes it by that agent's door", async () => {
    window.localStorage.setItem('meta-console-dock-state:["","console-storage:v1:two-root"]', JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "schedules", kind: "schedules", title: "Schedules" } }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
    const any = { kind: "any" };
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS),
      // As the hosted root's door writes it: its agent `triage`, sent as its agent `reviewer`.
      hostedSchedules: [{
        schedule_id: "sch-1", identity: "triage", name: "Morning sort", content: "Sort what came in overnight.",
        trigger: { type: "calendar", timezone: "UTC", minute: { kind: "values", values: [0] }, hour: { kind: "values", values: [9] }, day_of_month: any, month: any, day_of_week: any },
        phase: "active", revision: 1, sender: "reviewer",
      }],
    });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    const panel = await screen.findByTestId("pane:panel-1", undefined, { timeout: 8_000 });
    await waitFor(() => expect(panel.textContent).toContain("Morning sort"), { timeout: 8_000 });
    await act(async () => { fireEvent.click(await screen.findByTestId("schedules-view:list")); });
    await waitFor(() => expect(panel.textContent).toContain("Sends as Support reviewer"), { timeout: 5_000 });
    expect(panel.textContent).not.toContain("Launch reviewer");
    // The schedule is the hosted root's `triage`'s: it is shown under that agent.
    expect(panel.textContent).toContain("Triage");
    // Pausing it goes to that agent's door.
    await act(async () => { fireEvent.click(screen.getByTestId("schedules-pause:sch-1")); });
    await waitFor(() => expect(
      gateway.calls("meta/schedule/pause").map((request) => [request.params?.mob_id, request.params?.identity, request.params?.schedule_id]),
    ).toEqual([[HOSTED_MOB, "triage", "sch-1"]]), { timeout: 5_000 });
  }, 30_000);

  it("says why a schedule for a hosted root's agent was refused in words that are true for it and hold no mob id", async () => {
    window.localStorage.setItem('meta-console-dock-state:["","console-storage:v1:two-root"]', JSON.stringify({
      tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
      panels: [{ id: "panel-1", mode: "console", target: { id: "inspect:support/reviewer", kind: "identity-inspect", identity: "support/reviewer", memberId: "support/reviewer", title: "Details" } }],
      activeTabId: "tab-1",
      focusedPanelId: "panel-1",
    }));
    // The gateway sends schedules for the launch mob's agents only, and refuses one for the hosted root's agent.
    const gateway = createTwoRootGateway({ hostedSchedulesNeverSent: true, methods: Object.values(CONSOLE_RPC_METHODS) });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    fireEvent.click(await screen.findByTestId("schedules-new:support/reviewer", undefined, { timeout: 8_000 }));
    const form = await screen.findByTestId("schedule-form:support/reviewer");
    fireEvent.change(within(form).getByTestId("schedule-form-name"), { target: { value: "Morning review" } });
    fireEvent.change(within(form).getByTestId("schedule-form-content"), { target: { value: "Review what came in overnight." } });
    fireEvent.submit(form);
    const said = (await screen.findByTestId("schedules-error:support/reviewer", undefined, { timeout: 5_000 })).textContent || "";
    // The request reached the agent's own door.
    expect(gateway.calls("meta/schedule/create").map((request) => [request.params?.mob_id, request.params?.identity])).toEqual([[HOSTED_MOB, "reviewer"]]);
    // True whichever the reason (no schedule host at all, or one that sends for the launch team's
    // agents only): no schedules are sent for this team's agents. It claims nothing about other teams.
    expect(said).toBe("This gateway sends no schedules for agents of Support team, so a schedule for this agent would never be sent. Nothing was saved.");
    expect(said).not.toContain("Ops team");
    // No mob id: neither in the sentence nor anywhere on the page.
    expect(/\bsupport\b(?! team)/i.test(said)).toBe(false);
    expect(document.body.textContent || "").not.toContain("of mob support");
  }, 20_000);

  it("shows what a hosted root's agent did before the page opened, also when the roster of every mob answers after the stream's first page", async () => {
    const gateway = createTwoRootGateway({ capabilitiesDelayMs: 200, everyMobExperienceDelayMs: 700 });
    // Before the page is opened: an agent of the hosted root failed at something.
    gateway.push({
      mob: HOSTED_MOB, identity: "reviewer", kind: "interaction_failed", interactionId: "before-1", payload: { error: "The tool timed out." },
      // A moment ago: the rail shows the last minutes.
      extra: { timestamp_ms: Date.now() - 5_000 },
    });
    await start(gateway);
    // The capability arrives after the first roster; the roster of every mob answers late,
    // after the stream was opened again and read its first page.
    expect(await screen.findByTestId("sidebar-agent:support/reviewer", undefined, { timeout: 6_000 })).toBeTruthy();
    await settle(900);
    // The signals rail holds that failure, under the hosted root's agent.
    const rail = screen.getByTestId("signals-rail");
    await waitFor(() => expect(rail.textContent).toContain("Support reviewer"), { timeout: 4_000 });
    expect(rail.textContent).not.toContain("Launch reviewer");
  }, 20_000);

  it("reads its lists again when a frame names a mob that is in none of them", async () => {
    const gateway = createTwoRootGateway();
    await start(gateway);
    await settle(600);
    gateway.clear();
    // A frame that asks for no refresh by its kind.
    act(() => { gateway.push({ mob: "latecomer", identity: "reviewer", kind: "text_delta", payload: { delta: "from a mob not listed" } }); });
    await waitFor(() => expect(gateway.requests.filter((request) => request.route === "GET /console/experience").length).toBe(1), { timeout: 4_000 });
    // More frames of that mob ask for no more reads: the lists were read for it.
    gateway.clear();
    act(() => {
      gateway.push({ mob: "latecomer", identity: "reviewer", kind: "text_delta", payload: { delta: "more" } });
      gateway.push({ mob: "latecomer", identity: "reviewer", kind: "text_delta", payload: { delta: "and more" } });
    });
    await settle(900);
    expect(gateway.requests.filter((request) => request.route === "GET /console/experience")).toEqual([]);
  }, 20_000);
});
