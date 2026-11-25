import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB, type AgentRecordFamily, type TwoRootGateway } from "./fixtures/two-root-gateway";

// A change of an agent's record, made from the agent's Details. The
// gateway hosts two mobs made from one definition: each has an agent
// `reviewer` with the same record (the same spec digest). Whatever the
// person does to the hosted mob's reviewer goes to that mob's door, by the
// row the dialog was opened for; the launch mob's reviewer is untouched.
// A request that reached the launch mob's door with the bare name would be
// carried out there: the scripted gateway does exactly that.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const LONG = 30_000;
const HOSTED = "support/reviewer";
const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

/** The console with the Details of one agent open, on a gateway that keeps agents' records per door. */
async function openDetails(member: string): Promise<TwoRootGateway> {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", mode: "console", target: { id: `inspect:${member}`, kind: "identity-inspect", identity: member, memberId: member, title: "Details" } }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
  const gateway = createTwoRootGateway({ agentRecords: true });
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId(`inspect-record:${member}`, undefined, { timeout: 8_000 });
  return gateway;
}

function choose(member: string, change: "edit" | "archive" | "remove") {
  fireEvent.click(screen.getByTestId(`inspect-menu:${member}`));
  fireEvent.click(screen.getByTestId(`inspect-action:${member}:${change}`));
}

/** What one door's family was asked, by method. */
const asked = (family: AgentRecordFamily, method: string): Array<Record<string, unknown>> =>
  family.calls.filter((call: { method: string }) => call.method === method).map((call: { params: Record<string, unknown> }) => call.params);

/** The methods the launch mob's family was asked that are about one agent or one request. */
const launchAskedAboutAnAgent = (gateway: TwoRootGateway): string[] =>
  gateway.families!.launch.calls
    .map((call: { method: string }) => call.method)
    .filter((method: string) => !["meta/identity/list", "meta/identity/directory"].includes(method));

/** The mob and the agent each request of the family named, as sent. */
const sent = (gateway: TwoRootGateway, method: string) =>
  gateway.calls(method).map((request) => [request.params?.mob_id, request.params?.name]);

const reviewerOf = (family: AgentRecordFamily) => family.agents.get("reviewer");

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

describe("a change of a hosted mob's agent whose launch mob has an agent of the same name and record", () => {
  it("starts from two agents with one record: the same name and the same spec digest behind two doors", async () => {
    const gateway = createTwoRootGateway({ agentRecords: true });
    const digest = (family: AgentRecordFamily) => family.handle("meta/identity/get", { name: "reviewer" }).result.spec_digest;
    expect(digest(gateway.families!.launch)).toBe(digest(gateway.families!.hosted));
    gateway.families!.launch.calls.length = 0;
    gateway.families!.hosted.calls.length = 0;
  });

  it("archives the hosted mob's agent, by its door; the launch mob's agent is untouched", async () => {
    const gateway = await openDetails(HOSTED);
    choose(HOSTED, "archive");
    const confirm = await screen.findByTestId(`identity-confirm:${HOSTED}:archive`, undefined, { timeout: 5_000 });
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    await waitFor(() => expect(reviewerOf(gateway.families!.hosted).archived).toBe(true), { timeout: 6_000 });
    expect(reviewerOf(gateway.families!.launch).archived).toBe(false);
    expect(launchAskedAboutAnAgent(gateway)).toEqual([]);
    expect(sent(gateway, "meta/identity/archive")).toEqual([[HOSTED_MOB, "reviewer"]]);
    // The record's read and the plan before the change went the same way.
    for (const method of ["meta/identity/get", "meta/identity/plan"]) {
      expect(new Set(sent(gateway, method).map((pair) => JSON.stringify(pair))), method).toEqual(new Set([JSON.stringify([HOSTED_MOB, "reviewer"])]));
    }
  }, LONG);

  it("removes the hosted mob's agent, by its door; the launch mob's agent is untouched", async () => {
    const gateway = await openDetails(HOSTED);
    choose(HOSTED, "remove");
    const confirm = await screen.findByTestId(`identity-confirm:${HOSTED}:remove`, undefined, { timeout: 5_000 });
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    await waitFor(() => expect(gateway.families!.hosted.agents.has("reviewer")).toBe(false), { timeout: 6_000 });
    expect(gateway.families!.launch.agents.has("reviewer")).toBe(true);
    expect(reviewerOf(gateway.families!.launch).archived).toBe(false);
    expect(launchAskedAboutAnAgent(gateway)).toEqual([]);
    expect(sent(gateway, "meta/identity/remove")).toEqual([[HOSTED_MOB, "reviewer"]]);
  }, LONG);

  it("edits the hosted mob's agent, by its door, with that door's directory; the launch mob's agent is untouched", async () => {
    const gateway = await openDetails(HOSTED);
    choose(HOSTED, "edit");
    const edit = await screen.findByTestId(`identity-edit:${HOSTED}`, undefined, { timeout: 5_000 });
    const field = await within(edit).findByTestId("identity-field:instructions:0", undefined, { timeout: 5_000 });
    fireEvent.change(field, { target: { value: "Review the work twice." } });
    fireEvent.submit(edit);
    await waitFor(() => expect(reviewerOf(gateway.families!.hosted).instructions).toEqual(["Review the work twice."]), { timeout: 6_000 });
    expect(reviewerOf(gateway.families!.launch).instructions).toEqual(["Review the work."]);
    expect(launchAskedAboutAnAgent(gateway)).toEqual([]);
    expect(sent(gateway, "meta/identity/update")).toEqual([[HOSTED_MOB, "reviewer"]]);
    // The profiles and the peers the form offered are the hosted mob's.
    expect(gateway.calls("meta/identity/directory").map((request) => request.params?.mob_id)).toEqual([HOSTED_MOB]);
    expect(asked(gateway.families!.launch, "meta/identity/directory")).toEqual([]);
  }, LONG);

  it("reads and withdraws a held change of the hosted mob's agent by that agent's door", async () => {
    const gateway = await openDetails(HOSTED);
    // The hosted mob's rules hold a change to this profile for a person; the launch mob's allow it.
    gateway.families!.hosted.setProfileChange("reviewer", "require_approval");
    choose(HOSTED, "edit");
    const edit = await screen.findByTestId(`identity-edit:${HOSTED}`, undefined, { timeout: 5_000 });
    fireEvent.click(await within(edit).findByTestId("identity-profile:reviewer", undefined, { timeout: 5_000 }));
    // The plan is the hosted mob's: a person must decide.
    await waitFor(() => expect(screen.getByTestId("identity-profile-plan").getAttribute("data-plan")).toBe("require_approval"), { timeout: 5_000 });
    fireEvent.submit(edit);
    const notice = await screen.findByTestId("identity-notice", undefined, { timeout: 5_000 });
    await waitFor(() => expect(notice.querySelector("[data-outcome='pending_decision']")).not.toBeNull(), { timeout: 5_000 });
    const requestId = asked(gateway.families!.hosted, "meta/identity/update")[0].request_id;
    expect(reviewerOf(gateway.families!.hosted).profile).toBe("worker");

    // The console reads its lists again, and with them the held request: asked of the door that holds it.
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(gateway.calls("meta/identity/operation").length).toBeGreaterThan(0), { timeout: 5_000 });
    await settle(400);
    expect(gateway.calls("meta/identity/operation").map((request) => [request.params?.mob_id, request.params?.request_id]))
      .toEqual(gateway.calls("meta/identity/operation").map(() => [HOSTED_MOB, requestId]));
    expect(asked(gateway.families!.launch, "meta/identity/operation")).toEqual([]);

    // Withdraw: the door that holds the request is asked, and the request is no longer held there.
    fireEvent.click(within(screen.getByTestId("identity-notice")).getByTestId("identity-withdraw"));
    await waitFor(() => expect(screen.getByTestId("identity-notice").textContent).toContain("The request is withdrawn"), { timeout: 5_000 });
    expect(asked(gateway.families!.hosted, "meta/identity/withdraw")).toEqual([{ request_id: requestId }]);
    expect(asked(gateway.families!.launch, "meta/identity/withdraw")).toEqual([]);
    expect(gateway.families!.hosted.decisionStatuses()).toEqual([]);
    expect(launchAskedAboutAnAgent(gateway)).toEqual([]);
  }, LONG);
});

describe("a held change of a hosted mob's agent whose mob idled and went to sleep", () => {
  it("is not read again on the console's own, so the mob stays asleep; withdrawing it is the person's act", async () => {
    const gateway = await openDetails(HOSTED);
    gateway.families!.hosted.setProfileChange("reviewer", "require_approval");
    choose(HOSTED, "edit");
    const edit = await screen.findByTestId(`identity-edit:${HOSTED}`, undefined, { timeout: 5_000 });
    fireEvent.click(await within(edit).findByTestId("identity-profile:reviewer", undefined, { timeout: 5_000 }));
    await waitFor(() => expect(screen.getByTestId("identity-profile-plan").getAttribute("data-plan")).toBe("require_approval"), { timeout: 5_000 });
    fireEvent.submit(edit);
    const notice = await screen.findByTestId("identity-notice", undefined, { timeout: 5_000 });
    await waitFor(() => expect(notice.querySelector("[data-outcome='pending_decision']")).not.toBeNull(), { timeout: 5_000 });
    const requestId = asked(gateway.families!.hosted, "meta/identity/update")[0].request_id;

    // The mob unloads while the change waits for a person.
    gateway.set({ hostedState: "dormant" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 5_000 });
    await settle(700);
    gateway.clear();
    const wakes = gateway.wakes();

    // Every read of the lists from here on: the held change is not read, nothing names the mob.
    for (let turn = 0; turn < 2; turn += 1) {
      act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
      await settle(800);
    }
    expect(gateway.requests.some((request) => request.route === "GET /console/experience")).toBe(true);
    expect(gateway.calls("meta/identity/operation")).toEqual([]);
    expect(gateway.requests.filter((request) => "mob_id" in (request.params ?? {}) || request.query.some(([name]) => name === "mob_id"))).toEqual([]);
    expect(gateway.wakes()).toBe(wakes);
    expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep");

    // The person withdraws the request: that names the mob, wakes it, and the request is withdrawn there.
    fireEvent.click(within(screen.getByTestId("identity-notice")).getByTestId("identity-withdraw"));
    await waitFor(() => expect(screen.getByTestId("identity-notice").textContent).toContain("The request is withdrawn"), { timeout: 5_000 });
    expect(asked(gateway.families!.hosted, "meta/identity/withdraw")).toEqual([{ request_id: requestId }]);
    expect(gateway.wakes()).toBe(wakes + 1);
  }, LONG);
});

describe("the same change of the launch mob's agent of that name", () => {
  it("is sent as before hosted mobs: the bare name and no mob; the hosted mob's agent is untouched", async () => {
    const gateway = await openDetails("reviewer");
    choose("reviewer", "archive");
    const confirm = await screen.findByTestId("identity-confirm:reviewer:archive", undefined, { timeout: 5_000 });
    fireEvent.click(within(confirm).getByTestId("identity-confirm-yes"));
    await waitFor(() => expect(reviewerOf(gateway.families!.launch).archived).toBe(true), { timeout: 6_000 });
    expect(reviewerOf(gateway.families!.hosted).archived).toBe(false);
    expect(gateway.families!.hosted.calls).toEqual([]);
    for (const method of ["meta/identity/get", "meta/identity/plan", "meta/identity/archive"]) {
      expect(new Set(sent(gateway, method).map((pair) => JSON.stringify(pair))), method).toEqual(new Set([JSON.stringify([undefined, "reviewer"])]));
    }
  }, LONG);
});
