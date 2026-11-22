import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import { createTwoRootGateway, ensureAbortSignalAny, type TwoRootGatewayOptions } from "./fixtures/two-root-gateway";

// The host's lists (questions agents asked, approvals, the person's
// replies) name an agent by its bare name: an entry carries no mob. On a
// gateway that hosts mobs behind doors of their own such a name can be the
// launch mob's agent or another mob's agent of the same name, and the entry
// does not say which. The console then resolves it to no agent: the entry
// is placed in no chat, names its agent in plain words, and opens no agent.
// A name that can only be one agent, and every name on a gateway with one
// mob, is resolved as before.

vi.mock("./use-voice-controller", () => ({
  useVoiceController: vi.fn(() => ({ voice: null, state: { phase: "idle", target: null, microphoneMuted: false, speakerMuted: false, error: null, notice: null } })),
}));
vi.mock("../mcp-apps/native-host", () => ({
  createNativeMcpAppsHost: vi.fn(() => ({ sandboxProxyUrl: "https://sandbox.test/sandbox.html", resolve: async () => null })),
}));

const LONG = 30_000;
const DOCK_KEY = 'meta-console-dock-state:["","console-storage:v1:two-root"]';
const DIGEST = `sha256:${"b".repeat(64)}`;
const settle = (ms = 300) => act(async () => { await new Promise((resolve) => window.setTimeout(resolve, ms)); });

/**
 * A set of questions as the gateway lists it: asked by `by`, the name its
 * door knows it by. `mob` is the mob the entry carries beside the name (the
 * frame pair): absent for an entry that carries none.
 */
const questionsFrom = (by: string, pendingId = "q-1", mob?: string) => ({
  pending_id: pendingId,
  origin: { identity: by, ...(mob ? { mob_id: mob } : {}) },
  questions: [{ id: "q-region", prompt: "Which region goes first?", choices: [{ id: "c-eu", label: "Europe" }, { id: "c-us", label: "United States" }] }],
  created_at_ms: Date.now() - 60_000,
  deadline_at_ms: Date.now() + 3_600_000,
  subject_digest: DIGEST,
});

const approvalFrom = (by: string, mob?: string) => ({
  pending_id: "a-1", action_id: "tool.deploy", action: "Deploy the release", actor_id: by, status: "pending",
  created_at_ms: Date.now() - 30_000, deadline_at_ms: Date.now() + 3_600_000,
  ...(mob ? { origin: { identity: by, mob_id: mob } } : {}),
});

function savePane(target: Record<string, unknown>) {
  window.localStorage.setItem(DOCK_KEY, JSON.stringify({
    tabs: [{ id: "tab-1", presetId: "single", layout: { kind: "panel", panelId: "panel-1" } }],
    panels: [{ id: "panel-1", mode: "console", target }],
    activeTabId: "tab-1",
    focusedPanelId: "panel-1",
  }));
}
const chatWith = (identity: string, title: string) =>
  ({ id: identity, kind: "agent-chat", addressingMode: "identity", memberId: identity, identity, title });

async function consoleOn(options: TwoRootGatewayOptions) {
  const gateway = createTwoRootGateway({ methods: Object.values(CONSOLE_RPC_METHODS), ...options });
  vi.stubGlobal("fetch", gateway.fetch);
  render(<ConsoleApp baseUrl="" />);
  // The question sets and the approvals are read: the Approvals badge counts them.
  await waitFor(() => expect(screen.getByTestId("nav-badge:gating").textContent).not.toBe(""), { timeout: 8_000 });
  await settle(500);
  return gateway;
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

describe("a set of questions whose asker's name two mobs' agents share", () => {
  it("is not placed in the chat of the launch mob's agent of that name", async () => {
    savePane(chatWith("reviewer", "Launch reviewer"));
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer")] });
    const chat = await screen.findByTestId("chat-pane:reviewer");
    expect(chat.querySelector("[data-testid='question-set:q-1']")).toBeNull();
    expect(chat.textContent).not.toContain("Which region goes first?");
  }, LONG);

  it("is not placed in the chat of the hosted mob's agent of that name either: the entry does not say whose it is", async () => {
    savePane(chatWith("support/reviewer", "Support reviewer"));
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer")] });
    const chat = await screen.findByTestId("chat-pane:support/reviewer");
    expect(chat.querySelector("[data-testid='question-set:q-1']")).toBeNull();
  }, LONG);

  it("is answered in the Approvals view, where it names its asker in plain words and opens no agent", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer")] });
    const card = await screen.findByTestId("question-set:q-1", undefined, { timeout: 5_000 });
    expect(card.textContent).toContain("reviewer (team not named)");
    // Not the launch agent's name for people, nor the hosted agent's.
    expect(card.textContent).not.toContain("Launch reviewer");
    expect(card.textContent).not.toContain("Support reviewer");
    expect(Array.from(card.querySelectorAll("button")).filter((button) => /^Open .* chat$/.test(button.getAttribute("aria-label") || ""))).toEqual([]);
    // It can still be answered here: the answer goes to the set by its own id.
    expect(card.textContent).toContain("Which region goes first?");
  }, LONG);
});

describe("a pending approval whose agent's name two mobs' agents share", () => {
  it("is shown in Today with the agent in plain words, not as the launch mob's agent", async () => {
    savePane({ id: "timeline", kind: "timeline", title: "Today" });
    await consoleOn({ hostPendingApprovals: [approvalFrom("reviewer")] });
    const today = await screen.findByTestId("pane:panel-1");
    await waitFor(() => expect(today.textContent).toContain("asked for approval"), { timeout: 5_000 });
    // The words as they are: not written like an agent's name.
    expect(today.textContent).toContain("reviewer (team not named)");
    expect(today.textContent).not.toContain("Reviewer team not named");
    expect(today.textContent).not.toContain("Launch reviewer asked for approval");
    expect(today.textContent).not.toContain("Support reviewer asked for approval");
  }, LONG);
});

describe("opening the agent of such an approval, from Today and from the signals rail", () => {
  /** Every control of `scope` a person could press to open something. */
  const pressAll = async (scope: HTMLElement) => {
    for (const control of Array.from(scope.querySelectorAll("button, a, [role='button']"))) {
      await act(async () => { fireEvent.click(control); });
    }
    await settle(400);
  };
  const launchReviewerOpen = () =>
    document.querySelector("[data-testid='chat-pane:reviewer'], [data-testid='inspect-panel:reviewer']") !== null;

  it("opens no agent from Today: the row names the agent in plain words, with nothing to press on the name", async () => {
    savePane({ id: "timeline", kind: "timeline", title: "Today" });
    await consoleOn({ hostPendingApprovals: [approvalFrom("reviewer")] });
    const today = await screen.findByTestId("pane:panel-1");
    await waitFor(() => expect(today.textContent).toContain("asked for approval"), { timeout: 5_000 });
    const names = Array.from(today.querySelectorAll(".today__who")).filter((node) => (node.textContent || "").includes("reviewer (team not named)"));
    expect(names.length).toBeGreaterThan(0);
    expect(names.filter((node) => node.tagName === "BUTTON" || node.closest("button") !== null)).toEqual([]);
    for (const row of Array.from(today.querySelectorAll("[data-testid^='timeline-story-row:']"))) await pressAll(row as HTMLElement);
    expect(launchReviewerOpen()).toBe(false);
  }, LONG);

  it("opens no agent from the signals rail: its controls open the approval, never the launch mob's agent", async () => {
    savePane(chatWith("planner", "Planner"));
    await consoleOn({ hostPendingApprovals: [approvalFrom("reviewer")] });
    const signal = await screen.findByTestId("signal:single:approval:a-1", undefined, { timeout: 5_000 });
    expect(signal.textContent).toContain("reviewer (team not named)");
    expect(signal.textContent).not.toContain("Launch reviewer");
    await pressAll(signal);
    await act(async () => { fireEvent.click(signal); });
    await settle(400);
    expect(launchReviewerOpen()).toBe(false);
  }, LONG);
});

describe("a name in the host's lists that can only be one agent", () => {
  it("is resolved as before: the set is placed in that agent's chat", async () => {
    savePane(chatWith("planner", "Planner"));
    // Every mob the host lists is loaded, and only the launch mob has a `planner`.
    await consoleOn({ sleepers: false, hostQuestionSets: [questionsFrom("planner")] });
    const chat = await screen.findByTestId("chat-pane:planner");
    await waitFor(() => expect(chat.querySelector("[data-testid='question-set:q-1']")).not.toBeNull(), { timeout: 5_000 });
  }, LONG);

  it("is resolved as before on a gateway with one mob, also for a name that would be shared elsewhere", async () => {
    savePane(chatWith("reviewer", "Launch reviewer"));
    await consoleOn({ addressing: false, hosted: false, hostQuestionSets: [questionsFrom("reviewer")] });
    const chat = await screen.findByTestId("chat-pane:reviewer");
    await waitFor(() => expect(chat.querySelector("[data-testid='question-set:q-1']")).not.toBeNull(), { timeout: 5_000 });
  }, LONG);
});

// An entry that carries its mob beside the name (the pair of a frame) names
// one agent: it is placed by that pair. The contract says a pending gating
// entry and a set of questions (`meta/gating/questions`) carry the pair in
// `origin.mob_id` and `origin.identity`. The gateway writes a mob on a
// pending entry of another mob's member. A set it lists names its asker
// alone, so the rows about a set pin what the console does with a set that
// carries a mob.
describe("an entry of the host's lists that carries its mob beside the name", () => {
  const card = (chat: HTMLElement, pendingId: string) => chat.querySelector(`[data-testid='question-set:${pendingId}']`);

  it("is placed in the chat of that mob's agent, and not in the launch agent's of the same name", async () => {
    savePane(chatWith("support/reviewer", "Support reviewer"));
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer", "q-hosted", "support")] });
    const chat = await screen.findByTestId("chat-pane:support/reviewer");
    await waitFor(() => expect(card(chat, "q-hosted")).not.toBeNull(), { timeout: 5_000 });
    expect(card(chat, "q-hosted")?.textContent).toContain("Support reviewer");
    cleanup();

    savePane(chatWith("reviewer", "Launch reviewer"));
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer", "q-hosted", "support")] });
    const launch = await screen.findByTestId("chat-pane:reviewer");
    await settle(400);
    expect(card(launch, "q-hosted")).toBeNull();
  }, LONG);

  it("is told from an entry of the same name that carries no mob: one in its agent's chat, the other in no chat", async () => {
    // Two sets asked by `reviewer`: one says it is the hosted mob's, the other says nothing.
    const sets = [questionsFrom("reviewer", "q-hosted", "support"), questionsFrom("reviewer", "q-unsaid")];
    savePane(chatWith("support/reviewer", "Support reviewer"));
    await consoleOn({ hostQuestionSets: sets });
    const hosted = await screen.findByTestId("chat-pane:support/reviewer");
    await waitFor(() => expect(card(hosted, "q-hosted")).not.toBeNull(), { timeout: 5_000 });
    expect(card(hosted, "q-unsaid")).toBeNull();
    cleanup();

    // The entry with no mob is never read as the launch agent's where the name is shared.
    savePane(chatWith("reviewer", "Launch reviewer"));
    await consoleOn({ hostQuestionSets: sets });
    const launch = await screen.findByTestId("chat-pane:reviewer");
    await settle(400);
    expect(card(launch, "q-hosted")).toBeNull();
    expect(card(launch, "q-unsaid")).toBeNull();
    cleanup();

    // Both are answered in the Approvals view: one under its agent, one in plain words.
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    await consoleOn({ hostQuestionSets: sets });
    const named = await screen.findByTestId("question-set:q-hosted", undefined, { timeout: 5_000 });
    expect(named.textContent).toContain("Support reviewer");
    const unsaid = await screen.findByTestId("question-set:q-unsaid");
    expect(unsaid.textContent).toContain("reviewer (team not named)");
    expect(unsaid.textContent).not.toContain("Launch reviewer");
  }, 60_000);

  it("names its agent in Today when it is an approval, and opens that agent", async () => {
    savePane({ id: "timeline", kind: "timeline", title: "Today" });
    await consoleOn({ hostPendingApprovals: [approvalFrom("reviewer", "support")] });
    const today = await screen.findByTestId("pane:panel-1");
    await waitFor(() => expect(today.textContent).toContain("Support reviewer asked for approval"), { timeout: 5_000 });
    expect(today.textContent).not.toContain("Launch reviewer");
    const name = Array.from(today.querySelectorAll("button.today__who")).find((node) => (node.textContent || "").includes("Support reviewer"));
    expect(name).toBeTruthy();
    await act(async () => { fireEvent.click(name!); });
    expect(await screen.findByTestId("inspect-panel:support/reviewer", undefined, { timeout: 5_000 })).toBeTruthy();
    expect(screen.queryByTestId("inspect-panel:reviewer")).toBeNull();
  }, LONG);

  it("is resolved to no agent when the mob it carries is in none of the console's lists", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer", "q-elsewhere", "newcomer")] });
    const set = await screen.findByTestId("question-set:q-elsewhere", undefined, { timeout: 5_000 });
    expect(set.textContent).toContain("reviewer (team not named)");
    expect(set.textContent).not.toContain("newcomer");
    expect(set.textContent).not.toContain("Launch reviewer");
  }, LONG);
});

// The words name nobody to open, whatever else the console offers a link
// for: also where every name is offered one (a gateway that judges a
// request's mob and hosts no mob behind a door of its own), an entry whose
// mob the console does not list is offered no chat.
describe("a name held as words where every agent name is offered a link", () => {
  it("is offered no chat from a set of questions, and is written as the words", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    await consoleOn({ hosted: false, hostQuestionSets: [questionsFrom("PLANNER", "q-1"), questionsFrom("reviewer", "q-elsewhere", "newcomer")] });
    const opens = (set: HTMLElement) => Array.from(set.querySelectorAll("button")).filter((button) => /^Open .* chat$/.test(button.getAttribute("aria-label") || ""));
    // Every name is offered a link here: the launch agent's, by a looser spelling, is.
    const known = await screen.findByTestId("question-set:q-1", undefined, { timeout: 5_000 });
    expect(opens(known).length).toBe(1);
    const set = await screen.findByTestId("question-set:q-elsewhere", undefined, { timeout: 5_000 });
    expect(opens(set)).toEqual([]);
    expect(set.textContent).toContain("reviewer (team not named)");
    expect(set.textContent).not.toContain("Reviewer team not named");
  }, LONG);
});

// A gateway with one mob: a name in the host's lists that the console's
// directory does not hold by that very spelling is still offered as a link,
// as before hosted mobs, because the console's own opening finds the agent
// by a looser reading (case, a label). Where a gateway hosts other mobs a
// link is offered only for a name the directory holds.
describe("a name the directory does not hold by that spelling, on a gateway with one mob", () => {
  it("is still a link in Today, and opens the agent the console finds for it", async () => {
    savePane({ id: "timeline", kind: "timeline", title: "Today" });
    await consoleOn({ addressing: false, hosted: false, hostPendingApprovals: [approvalFrom("PLANNER")] });
    const today = await screen.findByTestId("pane:panel-1");
    await waitFor(() => expect(today.textContent).toContain("asked for approval"), { timeout: 5_000 });
    const link = today.querySelector("button.today__who");
    expect(link).not.toBeNull();
    await act(async () => { fireEvent.click(link!); });
    expect(await screen.findByTestId("inspect-panel:planner", undefined, { timeout: 5_000 })).toBeTruthy();
  }, LONG);

  it("is still offered its chat from a set of questions in the Approvals view", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    await consoleOn({ addressing: false, hosted: false, hostQuestionSets: [questionsFrom("PLANNER")] });
    const set = await screen.findByTestId("question-set:q-1", undefined, { timeout: 5_000 });
    const open = Array.from(set.querySelectorAll("button")).find((button) => /^Open .* chat$/.test(button.getAttribute("aria-label") || ""));
    expect(open).toBeTruthy();
    await act(async () => { fireEvent.click(open!); });
    expect(await screen.findByTestId("chat-pane:planner", undefined, { timeout: 5_000 })).toBeTruthy();
  }, LONG);
});

describe("a record of the gating audit whose agent's name two mobs' agents share", () => {
  const auditRecord = (auditId: string, by: string, mob?: string) => ({
    audit_id: auditId, action_id: "tool.deploy", action: "Deploy the release", decision: "approve", outcome: "approved",
    actor_id: by, timestamp_ms: Date.now() - 120_000,
    ...(mob ? { origin: { identity: by, mob_id: mob } } : {}),
  });

  it("names the agent in plain words where the record carries no mob, and as that mob's agent where it carries one", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    await consoleOn({
      hostAuditEntries: [auditRecord("audit-unsaid", "reviewer"), auditRecord("audit-hosted", "reviewer", "support")],
      // Something waits, so the Approvals badge shows when the lists are read.
      hostQuestionSets: [questionsFrom("planner")],
    });
    const view = await screen.findByTestId("pane:panel-1");
    await act(async () => { fireEvent.click(await screen.findByTestId("gating-tab:audit")); });
    await waitFor(() => expect(view.textContent).toContain("Deploy the release"), { timeout: 5_000 });
    // Open every record to read its facts.
    for (const toggle of Array.from(view.querySelectorAll("[data-testid^='gating-day:'] button[aria-expanded='false']"))) {
      await act(async () => { fireEvent.click(toggle); });
    }
    await waitFor(() => expect(view.textContent).toContain("Actor:"), { timeout: 5_000 });
    const actors = Array.from(view.querySelectorAll("*")).map((node) => (node.childElementCount === 0 ? node.textContent || "" : "")).filter((text) => text.startsWith("Actor:"));
    // The text as shown: the words as they are, and the hosted agent's name for people.
    expect(actors.sort()).toEqual(["Actor: Support reviewer", "Actor: reviewer (team not named)"]);
    expect(view.textContent).not.toContain("Launch reviewer");
  }, LONG);
});

describe("a name of the host's lists while the console does not yet hold the roster of every mob", () => {
  it("is resolved to no agent until that roster is held: any name could be another mob's agent", async () => {
    savePane(chatWith("planner", "Planner"));
    // Every mob is loaded and only the launch mob has a `planner`; the roster of every mob answers late.
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS), sleepers: false, everyMobExperienceDelayMs: 2_500,
      hostQuestionSets: [questionsFrom("planner")],
    });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await waitFor(() => expect(screen.getByTestId("nav-badge:gating").textContent).toBe("1"), { timeout: 2_000 });
    await settle(300);
    // The console knows the gateway hosts mobs of their own and has not read which yet.
    expect(gateway.requests.some((request) => request.route === "GET /console/experience" && request.query.some(([name]) => name === "mobs"))).toBe(true);
    expect(screen.queryByTestId("sidebar-agent:support/reviewer")).toBeNull();
    const chat = screen.getByTestId("chat-pane:planner");
    expect(chat.querySelector("[data-testid='question-set:q-1']")).toBeNull();

    // The roster of every mob is in: the name can only be the launch mob's `planner`.
    expect(await screen.findByTestId("sidebar-agent:support/reviewer", undefined, { timeout: 8_000 })).toBeTruthy();
    await waitFor(() => expect(chat.querySelector("[data-testid='question-set:q-1']")).not.toBeNull(), { timeout: 5_000 });
  }, LONG);
});

describe("any name in the host's lists while a hosted mob is asleep", () => {
  it("is resolved to no agent: the sleeping mob's agents are not known, so the name could be one of theirs", async () => {
    savePane(chatWith("planner", "Planner"));
    await consoleOn({ sleepers: false, hostedState: "dormant", hostQuestionSets: [questionsFrom("planner")] });
    const chat = await screen.findByTestId("chat-pane:planner");
    await settle(600);
    expect(chat.querySelector("[data-testid='question-set:q-1']")).toBeNull();
    // The Approvals count still holds it: nothing is lost, it is answered in the Approvals view.
    expect(screen.getByTestId("nav-badge:gating").textContent).toBe("1");
    fireEvent.click(screen.getByTestId("nav-badge:gating"));
  }, LONG);
});

// While the console does not know what the gateway hosts (no capability
// read has answered yet, or none ever does and the console goes on as with
// a gateway that hosts one mob) a name of the host's lists could be an
// agent of a mob the console has never heard of. Nothing of those lists is
// then placed in a chat. These rows pin it for the gateway whose launch mob
// and hosted mob both have a `reviewer`.
describe("the host's lists while the console does not know what the gateway hosts", () => {
  const LISTS = ["meta/gating/questions", "meta/gating/pending", "meta/gating/audit"] as const;
  const listReads = (gateway: { calls(method: string): unknown[] }) => LISTS.reduce((count, method) => count + gateway.calls(method).length, 0);
  const setInLaunchChat = () => Boolean(screen.queryByTestId("chat-pane:reviewer")?.querySelector("[data-testid='question-set:q-1']"));
  const late = (): TwoRootGatewayOptions => ({
    methods: Object.values(CONSOLE_RPC_METHODS), capabilitiesDelayMs: 2_500,
    hostQuestionSets: [questionsFrom("reviewer")], hostPendingApprovals: [approvalFrom("reviewer")],
  });

  it("place no set of questions in a chat before a capability read answers", async () => {
    savePane(chatWith("reviewer", "Launch reviewer"));
    const gateway = createTwoRootGateway(late());
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    // The capability read is on its way and has not answered.
    await waitFor(() => expect(gateway.calls("meta/capabilities").length).toBeGreaterThan(0), { timeout: 2_000 });
    const placed: boolean[] = [];
    for (let turn = 0; turn < 12; turn += 1) {
      await settle(120);
      placed.push(setInLaunchChat());
    }
    expect(placed).not.toContain(true);

    // It answers: the lists are counted, and the set of the shared name is still in no chat.
    await waitFor(() => expect(screen.getByTestId("nav-badge:gating").textContent).toBe("2"), { timeout: 8_000 });
    await settle(600);
    expect(setInLaunchChat()).toBe(false);
  }, LONG);

  it("place no set of questions in a chat while no capability read answers, also once the console goes on as with one mob", async () => {
    savePane(chatWith("reviewer", "Launch reviewer"));
    const gateway = createTwoRootGateway({ ...late(), capabilitiesDelayMs: 0, capabilitiesFailures: 1_000 });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await screen.findByTestId("sidebar-agent:planner");
    // Many reads of the roster follow (each frame asks for one): the console
    // asks for the capabilities a few more times, then goes on without.
    for (let turn = 0; turn < 8; turn += 1) {
      act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
      await settle(350);
    }
    // It went on as with a gateway that hosts one mob: the kept pane is
    // restored from the launch mob's roster, and no agent of another mob is listed.
    const chat = await screen.findByTestId("chat-pane:reviewer", undefined, { timeout: 5_000 });
    expect(screen.queryByTestId("sidebar-agent:support/reviewer")).toBeNull();
    await settle(800);
    expect(setInLaunchChat()).toBe(false);
    expect(chat.textContent).not.toContain("Which region goes first?");
  }, LONG);

  it("are read only once a capability read has answered: a command reads the capabilities first", async () => {
    const gateway = createTwoRootGateway(late());
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    await waitFor(() => expect(gateway.calls("meta/capabilities").length).toBeGreaterThan(0), { timeout: 2_000 });
    const reads: number[] = [];
    for (let turn = 0; turn < 12; turn += 1) {
      await settle(120);
      reads.push(listReads(gateway));
    }
    expect(Math.max(...reads)).toBe(0);
    await waitFor(() => expect(screen.getByTestId("nav-badge:gating").textContent).toBe("2"), { timeout: 8_000 });
    expect(listReads(gateway)).toBeGreaterThan(0);
  }, LONG);
});

// The lists of the host that name agents answer, of the entries agents
// made, those of the launch mob's agents alone unless the request asks for
// every mob. The contract names five; these rows are about the four of
// them the console reads (pending approvals, the gating audit, the
// questions agents asked, the person's replies): it has no read of the
// console audit log, the fifth.
// A console that knows the gateway hosts mobs behind doors of their own
// asks each of them for every mob; the list of held changes names no agent
// and is asked as before.
describe("the lists of the host that name agents, on a gateway that hosts mobs behind doors of their own", () => {
  const FOUR = ["meta/gating/pending", "meta/gating/audit", "meta/gating/questions", "meta/console/attention"] as const;
  const selectors = (gateway: { calls(method: string): Array<{ params: Record<string, unknown> }> }, method: string) =>
    gateway.calls(method).map((call) => call.params.mobs);

  it("are each asked for every mob; the list of held changes is not", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    const gateway = await consoleOn({ agentRecords: true, hostQuestionSets: [questionsFrom("planner")] });
    // The audit is read where it is shown.
    await act(async () => { fireEvent.click(await screen.findByTestId("gating-tab:audit")); });
    await waitFor(() => { for (const method of FOUR) expect(gateway.calls(method).length).toBeGreaterThan(0); }, { timeout: 8_000 });
    for (const method of FOUR) {
      expect([method, [...new Set(selectors(gateway, method))]]).toEqual([method, ["*"]]);
    }
    expect([...new Set(selectors(gateway, "meta/decisions/list"))].filter((selector) => selector !== undefined)).toEqual([]);
  }, LONG);

  it("so hold the entries of another mob's agents: a hosted agent's set of questions, served only to a request for every mob, is in that agent's chat", async () => {
    savePane(chatWith("support/reviewer", "Support reviewer"));
    await consoleOn({ hostQuestionSets: [questionsFrom("reviewer", "q-1", "support")] });
    const chat = await screen.findByTestId("chat-pane:support/reviewer");
    await waitFor(() => expect(chat.querySelector("[data-testid='question-set:q-1']")).not.toBeNull(), { timeout: 5_000 });
  }, LONG);

  it("are asked for every mob from their first read on, also where the capability is known late: a command reads the capabilities first, so none is read without the word", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    const gateway = createTwoRootGateway({
      methods: Object.values(CONSOLE_RPC_METHODS), capabilitiesDelayMs: 1_500,
      hostQuestionSets: [questionsFrom("reviewer", "q-1", "support")],
    });
    vi.stubGlobal("fetch", gateway.fetch);
    render(<ConsoleApp baseUrl="" />);
    // The hosted agent's set is counted with the first read of the list: no later read is needed for it.
    await waitFor(() => expect(screen.getByTestId("nav-badge:gating").textContent).toBe("1"), { timeout: 10_000 });
    expect(gateway.calls("meta/gating/questions").length).toBe(1);
    await act(async () => { fireEvent.click(await screen.findByTestId("gating-tab:audit")); });
    await waitFor(() => { for (const method of FOUR) expect(gateway.calls(method).length).toBeGreaterThan(0); }, { timeout: 8_000 });
    for (const method of FOUR) {
      expect([method, [...new Set(selectors(gateway, method))]]).toEqual([method, ["*"]]);
    }
  }, LONG);

  it("are asked as before on a gateway with one mob: no selector", async () => {
    savePane({ id: "gating", kind: "gating", title: "Approvals" });
    const gateway = await consoleOn({ addressing: false, hosted: false, hostQuestionSets: [questionsFrom("planner")] });
    await act(async () => { fireEvent.click(await screen.findByTestId("gating-tab:audit")); });
    await waitFor(() => { for (const method of FOUR) expect(gateway.calls(method).length).toBeGreaterThan(0); }, { timeout: 8_000 });
    for (const method of FOUR) {
      expect([method, selectors(gateway, method).filter((selector) => selector !== undefined)]).toEqual([method, []]);
    }
  }, LONG);
});

// A read of the roster is two reads (the experience and the modules), and
// the page takes the new roster only when both have answered. The
// connection has the new document from the first answer on. What the
// console knows of the roster it SHOWS does not change in between: a set of
// questions stays in its chat, with the choices made so far.
describe("the host's lists while a newer read of the roster is on its way to the page", () => {
  it("keep a set of questions in its chat, with its draft, between the answer of the experience and that of the modules", async () => {
    savePane(chatWith("planner", "Planner"));
    // Every mob is loaded and only the launch mob has a `planner`.
    const gateway = await consoleOn({ sleepers: false, hostQuestionSets: [questionsFrom("planner")] });
    const chat = await screen.findByTestId("chat-pane:planner");
    await waitFor(() => expect(chat.querySelector("[data-testid='question-set:q-1']")).not.toBeNull(), { timeout: 5_000 });
    const europe = () => chat.querySelector<HTMLInputElement>("[data-testid='question:q-1:q-region'] input[value='c-eu']");
    await act(async () => { fireEvent.click(europe()!); });
    expect(europe()?.checked).toBe(true);
    await settle(300);

    // From now on the modules answer well after the experience. A frame asks
    // for a read of the roster; more frames arrive while the modules are out.
    gateway.set({ modulesDelayMs: 1_500 });
    const experienceReads = () => gateway.requests.filter((request) => request.route === "GET /console/experience").length;
    const before = experienceReads();
    act(() => { gateway.push({ identity: "planner", kind: "state_changed", payload: {} }); });
    await waitFor(() => expect(experienceReads()).toBeGreaterThan(before), { timeout: 3_000 });
    const held: boolean[] = [];
    for (let turn = 0; turn < 8; turn += 1) {
      await settle(120);
      act(() => { gateway.push({ identity: "planner", kind: "assistant_text_delta", payload: { text: `word ${turn} ` } }); });
      await settle(20);
      held.push(chat.querySelector("[data-testid='question-set:q-1']") !== null);
    }
    expect(held).not.toContain(false);
    // The modules are in, the page has the new roster: the choice is still made.
    await settle(1_200);
    expect(chat.querySelector("[data-testid='question-set:q-1']")).not.toBeNull();
    expect(europe()?.checked).toBe(true);
  }, LONG);
});
