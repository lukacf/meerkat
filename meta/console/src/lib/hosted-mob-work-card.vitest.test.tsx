import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import {
  createTwoRootGateway, ensureAbortSignalAny, HOSTED_MOB,
  type FakeWorkItem, type RecordedRequest, type TwoRootGateway, type TwoRootGatewayOptions,
} from "./fixtures/two-root-gateway";

// A work card in the chat of a member of a hosted root belongs to that
// root's work graph. Both roots have a member `reviewer`, and both work
// graphs hold an item `work-1`: an item id is a token of its own graph, and
// two graphs can hold the same one. What the card sends (an action, the
// read after a conflict, the read when the chat opens) goes through the
// door of the card's own member, and never to the launch mob's graph.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const ITEM = "work-1";
const LAUNCH_ITEM: FakeWorkItem = { id: ITEM, title: "Launch item", status: "open", revision: 7 };
const HOSTED_ITEM: FakeWorkItem = { id: ITEM, title: "Support item", status: "open", revision: 3 };
const CLAIM = `workgraph-action:${ITEM}:claim`;

const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

const wireItem = (item: FakeWorkItem) => ({
  ...item, realm_id: "realm-1", namespace: "default", priority: "medium",
  created_at: "2026-07-08T09:00:00Z", updated_at: "2026-07-08T09:00:00Z",
});

/** A turn of `reviewer` behind `mob`'s door (the launch mob's without one) that made the work item `item`. */
function pushCard(gateway: TwoRootGateway, mob: string | undefined, item: FakeWorkItem) {
  const turn = `turn-${mob ?? "launch"}`;
  const frame = (kind: string, payload: Record<string, unknown>) =>
    gateway.push({ ...(mob ? { mob } : {}), identity: "reviewer", kind, interactionId: turn, payload });
  frame("interaction_started", { content: "Track the work", prompt: "Track the work" });
  frame("tool_call_requested", { id: `${turn}-call`, name: "workgraph_create", args: { title: item.title } });
  frame("tool_execution_completed", { id: `${turn}-call`, name: "workgraph_create", result: JSON.stringify({ item: wireItem(item) }) });
  frame("interaction_complete", {});
}

/** A gateway whose two work graphs both hold `work-1`, with a card of it in each `reviewer`'s chat. */
function gatewayWithCards(extra: Partial<TwoRootGatewayOptions> = {}): TwoRootGateway {
  const gateway = createTwoRootGateway({
    // Every method is listed, so the console offers the work graph's actions.
    methods: Object.values(CONSOLE_RPC_METHODS),
    sleepers: false,
    experience: { workgraph: { available: true, can_view: true, can_manage: true } },
    workGraph: { launch: [{ ...LAUNCH_ITEM }], hosted: [{ ...HOSTED_ITEM }] },
    ...extra,
  });
  pushCard(gateway, undefined, LAUNCH_ITEM);
  pushCard(gateway, HOSTED_MOB, HOSTED_ITEM);
  return gateway;
}

async function start(gateway: TwoRootGateway) {
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  await screen.findByTestId("chat-composer:planner");
  await settle(500);
}

/** The chat of `identity`, opened, with its card's Claim. */
async function openCard(identity: string) {
  await act(async () => { fireEvent.click(await screen.findByTestId(`sidebar-agent:${identity}`)); });
  await screen.findByTestId(`chat-composer:${identity}`);
  await screen.findByTestId(CLAIM);
  await settle(500);
}

async function claim() {
  await act(async () => { fireEvent.click(screen.getByTestId(CLAIM)); });
  await settle(400);
}

const params = (request: RecordedRequest | undefined) => request?.params ?? {};
/** What a WorkGraph request names: its mob, its item and the revision it expects. */
const named = (request: RecordedRequest) => ({ mob_id: params(request).mob_id, id: params(request).id, expected_revision: params(request).expected_revision });

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

describe("a work card in the chat of a member of a hosted root", () => {
  it("sends its action through that root's door, and leaves the launch mob's item of the same id as it was", async () => {
    const gateway = gatewayWithCards();
    await start(gateway);
    await openCard("support/reviewer");
    await claim();
    expect(gateway.calls("meta/workgraph/claim").map(named)).toEqual([{ mob_id: HOSTED_MOB, id: ITEM, expected_revision: HOSTED_ITEM.revision }]);
    expect(gateway.workItems("hosted")).toEqual([{ ...HOSTED_ITEM, status: "in_progress", revision: HOSTED_ITEM.revision + 1 }]);
    expect(gateway.workItems("launch")).toEqual([LAUNCH_ITEM]);
  });

  it("is read again from its own root when its chat opens, and shows nothing of the launch mob's item", async () => {
    const gateway = gatewayWithCards();
    await start(gateway);
    await openCard("support/reviewer");
    const snapshots = gateway.calls("meta/workgraph/snapshot");
    expect(snapshots.length > 0).toBe(true);
    expect([...new Set(snapshots.map((request) => params(request).mob_id))]).toEqual([HOSTED_MOB]);
    expect(screen.queryAllByText(LAUNCH_ITEM.title).length).toBe(0);
    // The card names its item in more than one place.
    expect(screen.queryAllByText(HOSTED_ITEM.title).length > 0).toBe(true);
  });

  it("is read again from its own root after a conflict, and its next action names that root's revision", async () => {
    const gateway = gatewayWithCards();
    await start(gateway);
    await openCard("support/reviewer");
    // Another person moved the hosted root's item on since the card was drawn.
    gateway.set({ workGraph: { launch: [{ ...LAUNCH_ITEM }], hosted: [{ ...HOSTED_ITEM, revision: 5 }] } });
    await claim();
    await waitFor(() => expect(gateway.calls("meta/workgraph/get").length > 0).toBe(true));
    await settle(400);
    expect(gateway.calls("meta/workgraph/get").map(named)).toEqual([{ mob_id: HOSTED_MOB, id: ITEM, expected_revision: undefined }]);
    await claim();
    expect(gateway.calls("meta/workgraph/claim").map(named)).toEqual([
      { mob_id: HOSTED_MOB, id: ITEM, expected_revision: HOSTED_ITEM.revision },
      { mob_id: HOSTED_MOB, id: ITEM, expected_revision: 5 },
    ]);
    expect(gateway.workItems("hosted")).toEqual([{ ...HOSTED_ITEM, status: "in_progress", revision: 6 }]);
    expect(gateway.workItems("launch")).toEqual([LAUNCH_ITEM]);
  });

  it("is refused by a door that does not serve its action, and the action reaches no other mob", async () => {
    const gateway = gatewayWithCards({ hostedWorkGraphChangesRefused: true });
    await start(gateway);
    await openCard("support/reviewer");
    const reads = gateway.calls("meta/workgraph/get").length;
    await claim();
    expect(gateway.calls("meta/workgraph/claim").map(named)).toEqual([{ mob_id: HOSTED_MOB, id: ITEM, expected_revision: HOSTED_ITEM.revision }]);
    expect(screen.queryByTestId("console-action-error") !== null).toBe(true);
    // A refusal is no conflict: nothing is read again, from any mob.
    expect(gateway.calls("meta/workgraph/get").length).toBe(reads);
    expect(gateway.workItems("hosted")).toEqual([HOSTED_ITEM]);
    expect(gateway.workItems("launch")).toEqual([LAUNCH_ITEM]);
  });

  it("is not read again while its root is asleep, and is read from that root once a person opened it again", async () => {
    // The card arrives after the chat opened: no read of the cards was due yet.
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS),
      sleepers: false,
      experience: { workgraph: { available: true, can_view: true, can_manage: true } },
      workGraph: { launch: [{ ...LAUNCH_ITEM }], hosted: [{ ...HOSTED_ITEM }] },
    });
    await start(gateway);
    await act(async () => { fireEvent.click(await screen.findByTestId("sidebar-agent:support/reviewer")); });
    await screen.findByTestId("chat-composer:support/reviewer");
    await settle(500);
    act(() => { pushCard(gateway, HOSTED_MOB, HOSTED_ITEM); });
    await screen.findByTestId(CLAIM);
    expect(gateway.calls("meta/workgraph/snapshot")).toEqual([]);
    // The root falls asleep: the console reads nothing of its work graph on
    // its own, from that root or from any other, and does not wake it.
    gateway.set({ hostedState: "dormant" });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.getByTestId("sidebar-mob-state:support").textContent).toBe("Asleep"), { timeout: 4_000 });
    await settle(800);
    const wakes = gateway.wakes();
    expect(gateway.calls("meta/workgraph/snapshot")).toEqual([]);
    // A person opens the root: the read that was held back is made now, once.
    await act(async () => { fireEvent.click(screen.getByTestId("sidebar-mob-toggle:support")); });
    await waitFor(() => expect(gateway.calls("meta/workgraph/snapshot").length).toBe(1), { timeout: 4_000 });
    await settle(600);
    expect(gateway.calls("meta/workgraph/snapshot").map((request) => params(request).mob_id)).toEqual([HOSTED_MOB]);
    expect(gateway.wakes()).toBe(wakes + 1);
  }, 15_000);

  it("is offered no action once its root is no longer listed, and sends none", async () => {
    const gateway = gatewayWithCards();
    await start(gateway);
    await openCard("support/reviewer");
    // What was read of any work graph up to here.
    const reads = gateway.calls("meta/workgraph/get").length + gateway.calls("meta/workgraph/snapshot").length;
    // The host no longer lists the root: the console has no door for the card's member.
    gateway.set({ hosted: false });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.queryByTestId("sidebar-mob:support")).toBeNull(), { timeout: 4_000 });
    await settle(600);
    // The chat of a member the console no longer lists is closed: its card
    // is gone with it, so no action of it can be sent as another mob's.
    expect(screen.queryByTestId("chat-composer:support/reviewer")).toBeNull();
    expect(screen.queryByTestId(CLAIM)).toBeNull();
    expect(gateway.calls("meta/workgraph/claim").map(named)).toEqual([]);
    expect(gateway.calls("meta/workgraph/get").length + gateway.calls("meta/workgraph/snapshot").length).toBe(reads);
    expect(gateway.workItems("launch")).toEqual([LAUNCH_ITEM]);
  }, 15_000);

  it("is not read from another mob when its root leaves the list before its first read was due", async () => {
    // The card arrives after the chat opened: no read of the cards was made yet.
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS),
      sleepers: false,
      experience: { workgraph: { available: true, can_view: true, can_manage: true } },
      workGraph: { launch: [{ ...LAUNCH_ITEM }], hosted: [{ ...HOSTED_ITEM }] },
    });
    await start(gateway);
    await act(async () => { fireEvent.click(await screen.findByTestId("sidebar-agent:support/reviewer")); });
    await screen.findByTestId("chat-composer:support/reviewer");
    await settle(500);
    act(() => { pushCard(gateway, HOSTED_MOB, HOSTED_ITEM); });
    await screen.findByTestId(CLAIM);
    expect(gateway.calls("meta/workgraph/snapshot")).toEqual([]);
    gateway.set({ hosted: false });
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(screen.queryByTestId("sidebar-mob:support")).toBeNull(), { timeout: 4_000 });
    await settle(800);
    // No work graph was read for the card: not its own root's, which is gone, and no other mob's.
    expect(gateway.calls("meta/workgraph/snapshot")).toEqual([]);
    expect(gateway.calls("meta/workgraph/get")).toEqual([]);
  }, 15_000);
});

describe("a work card in the chat of a member of the launch mob", () => {
  it("sends its action and its reads with no mob, and leaves the hosted root's item of the same id as it was", async () => {
    const gateway = gatewayWithCards();
    await start(gateway);
    await openCard("reviewer");
    expect([...new Set(gateway.calls("meta/workgraph/snapshot").map((request) => params(request).mob_id))]).toEqual([undefined]);
    await claim();
    expect(gateway.calls("meta/workgraph/claim").map(named)).toEqual([{ mob_id: undefined, id: ITEM, expected_revision: LAUNCH_ITEM.revision }]);
    expect(gateway.workItems("launch")).toEqual([{ ...LAUNCH_ITEM, status: "in_progress", revision: LAUNCH_ITEM.revision + 1 }]);
    expect(gateway.workItems("hosted")).toEqual([HOSTED_ITEM]);
  });
});
