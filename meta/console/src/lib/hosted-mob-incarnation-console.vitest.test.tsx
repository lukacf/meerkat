import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { createTwoRootGateway, ensureAbortSignalAny, type TwoRootGateway } from "./fixtures/two-root-gateway";
import { consoleSendStorageKey } from "./send-attempt-storage";

// A mob created again under the id of a destroyed one is another mob. The
// console drops what the browser kept for the old one (a draft, a queued
// message, an open chat) before it shows the new one, and it does the same
// for a mob that left its list. What it keeps for other mobs stays, and so
// does everything while a mob is only asleep.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });
/** Each row opens the console, keeps work in it and waits for a refresh of its lists. */
const LONG = 20_000;

async function open(identity: string) {
  await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${identity}`)); });
  return await screen.findByTestId(`chat-composer:${identity}`) as HTMLTextAreaElement;
}

async function draft(identity: string, text: string) {
  const textarea = await open(identity);
  await act(async () => { fireEvent.change(textarea, { target: { value: text } }); });
  await settle(600);
}

async function start(gateway: TwoRootGateway) {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId("chat-composer:planner");
  await settle(500);
}

const keys = (storage: Storage) => Array.from({ length: storage.length }, (_, index) => storage.key(index) || "");
/** The keys either storage holds for a member whose key starts with `<mob>/`. */
const keysOfMob = (mob: string) => [...keys(window.localStorage), ...keys(window.sessionStorage)].filter((key) => key.includes(`${mob}%2F`));
/** The scope the console keeps its queues and drafts under, read from a draft's key. */
const sendNamespace = () => {
  const draftKey = keys(window.sessionStorage).find((key) => key.startsWith("meta-composer-draft:v2:"));
  return decodeURIComponent((draftKey || "").split(":")[2] || "");
};
/** A frame of the launch mob that makes the console read its lists again. */
const refresh = (gateway: TwoRootGateway) => act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });

/**
 * The console with a draft for a member of the hosted root and a queued
 * message for another, a queued message for a member of the mob nested
 * under it, and a draft and a queued message for members of the launch mob.
 */
async function consoleWithKeptWork(gateway: TwoRootGateway) {
  await start(gateway);
  await draft("reviewer", "a draft for the launch reviewer");
  await draft("support/reviewer", "a draft for the hosted reviewer");
  const namespace = sendNamespace();
  expect(namespace).not.toBe("");
  window.localStorage.setItem(consoleSendStorageKey(namespace, "support/triage"), "{}");
  window.localStorage.setItem(consoleSendStorageKey(namespace, "support/support-lab/tester"), "{}");
  window.localStorage.setItem(consoleSendStorageKey(namespace, "planner"), "{}");
  // The nested mob's member is keyed under its root's door, so its keys are the root's too.
  expect(keysOfMob("support").length).toBeGreaterThanOrEqual(3);
  expect(keysOfMob("support%2Fsupport-lab").length).toBeGreaterThanOrEqual(1);
  return namespace;
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

describe("a mob created again under its id", () => {
  it("starts with no draft, no queued message and no open chat of the mob before it", async () => {
    const gateway = createTwoRootGateway();
    const namespace = await consoleWithKeptWork(gateway);

    gateway.set({ hostedIncarnation: "support-incarnation-2" });
    refresh(gateway);
    await waitFor(() => expect(keysOfMob("support")).toEqual([]), { timeout: 4_000 });
    // The chat of the old mob's member is closed, and the member of the new one starts empty.
    await waitFor(() => expect(screen.queryByTestId("chat-composer:support/reviewer")).toBeNull());
    expect((await open("support/reviewer")).value).toBe("");
    // What the browser keeps for the launch mob stays. The members of the mob
    // nested under the root are keyed behind the root's door (contract v0.6.0,
    // row_key), so they go with the root created again.
    expect(window.localStorage.getItem(consoleSendStorageKey(namespace, "planner"))).toBe("{}");
    expect(
      keysOfMob("support%2Fsupport-lab"),
      "a mob created again under its id is a new incarnation (the listed incarnation token changed): the gateway released "
        + "the old mob's schedules, routes, frames and memory, so nothing kept for a member behind its door carries over, "
        + "a member of a mob nested under it included",
    ).toEqual([]);
    expect((await open("reviewer")).value).toBe("a draft for the launch reviewer");
  }, LONG);
});

describe("a mob that left the console's list", () => {
  it("leaves nothing of it in the browser, and nothing of the mob nested under it", async () => {
    const gateway = createTwoRootGateway();
    const namespace = await consoleWithKeptWork(gateway);

    gateway.set({ hosted: false });
    refresh(gateway);
    await waitFor(() => expect([...keysOfMob("support"), ...keysOfMob("support-lab")]).toEqual([]), { timeout: 4_000 });
    expect(window.localStorage.getItem(consoleSendStorageKey(namespace, "planner"))).toBe("{}");
  }, LONG);
});

describe("a mob behind the launch mob's door that one roster does not list", () => {
  it("keeps everything the browser holds for it: it is listed only while it is held and one of its members is seen", async () => {
    const gateway = createTwoRootGateway();
    const namespace = await consoleWithKeptWork(gateway);
    window.localStorage.setItem(consoleSendStorageKey(namespace, "research/scout"), "{}");
    const before = keysOfMob("research").sort();
    expect(before.length).toBeGreaterThanOrEqual(1);

    // One read of the roster comes without the mob a launch member created.
    gateway.set({ launchNested: false });
    refresh(gateway);
    await waitFor(() => expect(screen.queryByTestId("sidebar-mob:research")).toBeNull(), { timeout: 4_000 });
    await settle(600);
    expect(keysOfMob("research").sort()).toEqual(before);
    // The next read lists it again, the same mob: still everything there.
    gateway.set({ launchNested: true });
    refresh(gateway);
    await waitFor(() => expect(screen.queryByTestId("sidebar-mob:research")).not.toBeNull(), { timeout: 4_000 });
    await settle(400);
    expect(keysOfMob("research").sort()).toEqual(before);
  }, LONG);
});

describe("a mob that went to sleep", () => {
  it("keeps what the browser holds for it and for the mob nested under it", async () => {
    const gateway = createTwoRootGateway();
    await consoleWithKeptWork(gateway);
    const before = [...keysOfMob("support"), ...keysOfMob("support-lab")].sort();

    gateway.set({ hostedState: "dormant" });
    refresh(gateway);
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    await settle(600);
    expect([...keysOfMob("support"), ...keysOfMob("support-lab")].sort()).toEqual(before);
    expect((screen.getByTestId("chat-composer:support/reviewer") as HTMLTextAreaElement).value).toBe("a draft for the hosted reviewer");
  }, LONG);
});

describe("signing out", () => {
  it("removes what this browser recorded of the mobs the user saw", async () => {
    const gateway = createTwoRootGateway({
      experience: { console_config: { title: "Operations", layout: { initial_agent: "planner" }, sign_out: { href: "/logout", method: "post" } } },
    });
    await start(gateway);
    const recorded = () => keys(window.localStorage).filter((key) => key.startsWith("meta-console-mob-incarnations:"));
    await waitFor(() => expect(recorded().length).toBe(1), { timeout: 4_000 });
    expect(window.localStorage.getItem(recorded()[0]) || "").toContain("support-incarnation-1");

    const chip = await screen.findByTestId("identity-chip");
    fireEvent.click(within(chip).getByRole("button", { name: /operator@example\.test/ }));
    const signOut = await screen.findByRole("button", { name: "Sign out" });
    const leave = vi.spyOn(HTMLFormElement.prototype, "submit").mockImplementation(() => {});
    fireEvent.submit(signOut.closest("form")!);
    await waitFor(() => expect(leave).toHaveBeenCalledTimes(1));
    await settle(400);
    // Neither the mobs' ids nor their tokens stay in the browser.
    expect(recorded()).toEqual([]);
    leave.mockRestore();
  }, LONG);
});
