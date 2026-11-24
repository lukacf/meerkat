import React from "react";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleWhoami, MetaConsoleTransport } from "./headless";

// The question sets agents ask, in the whole Console: what the sidebar
// counts, the card on the inbox's Pending tab, and what a person is shown
// when the gateway answers a decide. DOM assertions compare strings and
// booleans, never elements.

const identity = "identity:release-planner";
const PENDING_ID = `decision:${"a".repeat(64)}`;
const DIGEST = `sha256:${"b".repeat(64)}`;
const questionSet = () => ({
  pending_id: PENDING_ID,
  origin: { identity },
  title: "Release plan",
  questions: [{ id: "q-region", prompt: "Which region goes first?", choices: [{ id: "c-eu", label: "Europe" }, { id: "c-us", label: "United States" }] }],
  created_at_ms: Date.now() - 60_000,
  deadline_at_ms: Date.now() + 3_600_000,
  subject_digest: DIGEST,
});

function gateway(viewer: ConsoleWhoami, decide: (params: Record<string, unknown>) => unknown = () => ({})) {
  let sets: unknown[] | Error = [questionSet()];
  const decided: Array<Record<string, unknown>> = [];
  const fake: MetaConsoleTransport = {
    loadExperience: async () => ({
      runtime_id: "questions-test", storage_scope: "console-storage:v1:questions",
      console_config: { layout: { initial_agent: identity } },
      agent_sidebar: { live_snapshot: { agents: [{ identity, member_id: identity, agent_id: identity, label: "Release Planner", kind: "member", state: "running", addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false } }] } },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/gating/pending", "meta/gating/decide", "meta/gating/audit", "meta/gating/questions"] }) as never,
    whoami: async () => viewer,
    queryTimeline: async () => ({ frames: [], available: true }),
    subscribeTimeline: () => () => {},
    send: vi.fn(),
    executeCommand: async (input) => {
      if (input.command === "listGatingPending") return { command: input.command, accepted: true, result: { pending: [] } } as never;
      if (input.command === "listGatingAudit") return { command: input.command, accepted: true, result: { entries: [] } } as never;
      if (input.command === "listQuestionSets") {
        if (sets instanceof Error) throw sets;
        return { command: input.command, accepted: true, result: { question_sets: sets } } as never;
      }
      if (input.command === "decideGating") {
        const params = { ...(input.params as Record<string, unknown>) };
        decided.push(params);
        return { command: input.command, accepted: true, result: decide(params) } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
  return { fake, decided, list(next: unknown[] | Error) { sets = next; } };
}

const person: ConsoleWhoami = { actorKind: "person", principal: "a@example.test", approverIds: [], canAdminister: false, callerScope: "caller-a" };

beforeEach(() => {
  vi.stubGlobal("localStorage", window.localStorage);
  window.localStorage.clear();
  window.sessionStorage.clear();
});
afterEach(async () => {
  await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
  cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
});

describe("question sets in the Console", () => {
  it("counts an open question set in Needs you and on the Approvals view, and opens the inbox on its card", async () => {
    const { fake } = gateway(person);
    render(<ConsoleApp baseUrl="" transport={fake} />);
    fireEvent.click(await screen.findByRole("button", { name: "Needs you, 0 pending approvals, 1 question set" }));
    const card = await screen.findByTestId(`question-set:${PENDING_ID}`);
    expect(within(card).getByText("Release plan")).toBeVisible();
    expect(screen.getByTestId("question-set-list").contains(card)).toBe(true);
    await waitFor(() => expect(screen.getByTestId("nav-badge:gating").textContent).toBe("1"));
    expect(screen.getByTestId("approval-attention").querySelector(".cc-approval-attention__count")?.textContent).toBe("1");
  });

  const openInbox = async () => {
    fireEvent.click(await screen.findByRole("button", { name: /^Needs you/ }));
    return screen.findByTestId(`question-set:${PENDING_ID}`);
  };
  /** The words and ids of the set: none may be left on the page once the
   * gateway no longer lists it. (The agent's name stays: it is the roster's.) */
  const SET_WORDS = [PENDING_ID, DIGEST, "Release plan", "Which region goes first?", "Europe", "United States", "q-region", "c-eu", "c-us"];
  const rpcRefusal = (code: number, message: string, data?: Record<string, unknown>) => Object.assign(new Error(message), {
    rpcError: { code, message, ...(data ? { data } : {}) },
  });

  it("answers a set from the inbox through the decide command: the digest, the answers and no approver", async () => {
    const { fake, decided, list } = gateway(person, (params) => {
      list([]);
      return { pending_id: params.pending_id, kind: "question_set", outcome: "answered", decided_at_ms: Date.now() };
    });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const card = await openInbox();
    await waitFor(() => expect((within(card).getByRole("radio", { name: "Europe" }) as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(within(card).getByRole("radio", { name: "Europe" }));
    fireEvent.click(within(card).getByRole("button", { name: "Send answers" }));
    await waitFor(() => expect(screen.getByTestId(`question-set:${PENDING_ID}`).dataset.state).toBe("answered"));
    expect(decided.length).toBe(1);
    expect(decided[0]).toMatchObject({
      pending_id: PENDING_ID, decision: "approve", presented_digest: DIGEST,
      answers: [{ question_id: "q-region", answer: { kind: "choice", choice_id: "c-eu" } }],
    });
    expect("approver_id" in decided[0]).toBe(false);
    // Answered: it no longer counts as needing the person.
    await waitFor(() => expect(screen.queryByTestId("nav-badge:gating") === null).toBe(true));
  });

  it("takes a set off the page, with nothing of it left, when its decide is answered as an id that is not pending", async () => {
    const { fake, list } = gateway(person, () => {
      list([]);
      throw rpcRefusal(-32602, "gating decision failed");
    });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const card = await openInbox();
    await waitFor(() => expect((within(card).getByRole("radio", { name: "Europe" }) as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(within(card).getByRole("radio", { name: "Europe" }));
    fireEvent.click(within(card).getByRole("button", { name: "Send answers" }));
    await waitFor(() => expect(screen.queryAllByTestId(`question-set:${PENDING_ID}`).length).toBe(0));
    expect((await screen.findByTestId("question-set-notice")).textContent).toBe("This question is no longer listed here.");
    for (const word of SET_WORDS) expect(document.body.innerHTML.includes(word)).toBe(false);
    expect(document.body.textContent?.includes("gating decision failed")).toBe(false);
    expect(/not permitted|timed out/i.test(document.body.textContent ?? "")).toBe(false);
    await waitFor(() => expect(screen.queryByTestId("nav-badge:gating") === null).toBe(true));
  });

  it("says the gateway's typed refusal in the one line of a set that expired at its decide", async () => {
    const { fake, list } = gateway(person, () => {
      list([]);
      throw rpcRefusal(-32602, `decision ${PENDING_ID} refused the input: expired`, { kind: "decision_refused", decision_refusal: { kind: "expired" } });
    });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const card = await openInbox();
    await waitFor(() => expect((within(card).getByRole("button", { name: "Dismiss" }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(within(card).getByRole("button", { name: "Dismiss" }));
    await waitFor(() => expect(screen.queryAllByTestId(`question-set:${PENDING_ID}`).length).toBe(0));
    expect((await screen.findByTestId("question-set-notice")).textContent)
      .toBe("This question timed out before the gateway took your reply, so nothing was sent. If the answer still matters, tell the agent in its chat.");
    for (const word of SET_WORDS) expect(document.body.innerHTML.includes(word)).toBe(false);
  });

  it("says in Needs you and in the inbox that the questions could not be read, and reads them when asked to", async () => {
    const { fake, list } = gateway(person);
    list(new Error("gateway timeout"));
    render(<ConsoleApp baseUrl="" transport={fake} />);
    // The approvals were read (none waits): that count stays, and the words say what was not read.
    const needsYou = await screen.findByRole("button", { name: "Needs you, 0 pending approvals, Questions unavailable" });
    expect(screen.getByTestId("approval-attention").querySelector(".cc-approval-attention__count")?.textContent).toBe("0");
    fireEvent.click(needsYou);
    const notice = await screen.findByTestId("question-set-unavailable");
    expect(notice.textContent).toContain("The questions agents asked could not be loaded.");
    list([questionSet()]);
    fireEvent.click(within(notice).getByRole("button", { name: "Try again" }));
    expect(await screen.findByTestId(`question-set:${PENDING_ID}`)).toBeTruthy();
    expect(await screen.findByRole("button", { name: "Needs you, 0 pending approvals, 1 question set" })).toBeTruthy();
  });

  it("shows a caller who is not a signed-in person the question and what to do, and sends nothing", async () => {
    const { fake, decided } = gateway({ actorKind: "anonymous", approverIds: [], canAdminister: false, callerScope: "caller-open" });
    render(<ConsoleApp baseUrl="" transport={fake} />);
    const card = await openInbox();
    await waitFor(() => expect(card.dataset.state).toBe("read_only"));
    expect(within(card).getByText("Which region goes first?")).toBeVisible();
    expect((within(card).getByRole("button", { name: "Send answers" }) as HTMLButtonElement).disabled).toBe(true);
    expect((within(card).getByRole("button", { name: "Dismiss" }) as HTMLButtonElement).disabled).toBe(true);
    expect(decided.length).toBe(0);
  });
});

// An answered set in the asking agent's chat: the chat's active area (below
// the newest turn) holds what still needs the person, and what was answered
// stays with the turn it was answered after.
describe("an answered question set in the agent's chat", () => {
  const other = "identity:release-reviewer";
  const row = (id: string, label: string) => ({
    identity: id, member_id: id, agent_id: id, label, kind: "member", state: "running", addressable: true,
    affordances: { can_send_message: true }, model_capabilities: { image_input: false },
  });
  const turn = (id: string, atMs: number, ask: string, answer: string) => [
    { id: `${id}:ask`, event: "user_input", identity, interactionId: id, timestampMs: atMs, data: { content: ask }, sourceKind: "send" },
    { id: `${id}:done`, event: "interaction_complete", identity, interactionId: id, timestampMs: atMs + 1_000, data: { result: answer } },
  ];
  /** A gateway with two agents, one earlier turn of the asking agent, one
   * open set of it, and a stream the row writes a later turn to. Its clock
   * (the turns' times and the decision's) is `aheadMs` ahead of the page's. */
  function chatGateway(aheadMs = 0, moreSets: readonly unknown[] = []) {
    const clock = () => Date.now() + aheadMs;
    let sets: unknown[] = [questionSet(), ...moreSets];
    const decided: Array<Record<string, unknown>> = [];
    const decidedAt: number[] = [];
    const frames: Array<Record<string, unknown>> = [...turn("turn-1", clock() - 600_000, "Plan the release.", "One thing first: which region?")];
    const listeners = new Set<(frame: never) => void>();
    const fake: MetaConsoleTransport = {
      loadExperience: async () => ({
        runtime_id: "questions-chat-test", storage_scope: "console-storage:v1:questions-chat",
        console_config: { layout: { initial_agent: identity } },
        agent_sidebar: { live_snapshot: { agents: [row(identity, "Release Planner"), row(other, "Release Reviewer")] } },
        activity_feed: { filter_presets: [], active_preset_id: "all" },
      }) as never,
      loadModules: async () => ({ modules: [] }) as never,
      capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline", "meta/gating/pending", "meta/gating/decide", "meta/gating/audit", "meta/gating/questions"] }) as never,
      whoami: async () => person,
      queryTimeline: async (input) => ({
        frames: frames.filter((frame) => !input.identity || frame.identity === input.identity) as never,
        available: true,
      }),
      subscribeTimeline: (_input, onFrame) => {
        listeners.add(onFrame as never);
        return () => { listeners.delete(onFrame as never); };
      },
      send: vi.fn(),
      executeCommand: async (input) => {
        if (input.command === "listGatingPending") return { command: input.command, accepted: true, result: { pending: [] } } as never;
        if (input.command === "listGatingAudit") return { command: input.command, accepted: true, result: { entries: [] } } as never;
        if (input.command === "listQuestionSets") return { command: input.command, accepted: true, result: { question_sets: sets } } as never;
        if (input.command === "decideGating") {
          const params = { ...(input.params as Record<string, unknown>) };
          decided.push(params);
          sets = [];
          decidedAt.push(clock());
          return { command: input.command, accepted: true, result: { pending_id: params.pending_id, kind: "question_set", outcome: "answered", decided_at_ms: decidedAt[decidedAt.length - 1] } } as never;
        }
        return { command: input.command, accepted: true, result: {} } as never;
      },
    };
    const add = (id: string, atMs: number, ask: string, answer: string) => {
      const next = turn(id, atMs, ask, answer);
      frames.push(...next);
      for (const frame of next) for (const listener of listeners) listener(frame as never);
    };
    return {
      fake, decided, clock,
      /** The gateway no longer lists this set (it was resolved elsewhere). */
      drop(pendingId: string) { sets = sets.filter((set) => (set as { pending_id: string }).pending_id !== pendingId); },
      /** The gateway's own time of each decision it answered. */
      decidedAt,
      /** A turn of the asking agent that starts at `atMs` on the gateway's
       * clock: listed, and written to the stream when one is open. */
      add,
      /** A later turn of the asking agent arrives on the stream. */
      later(id: string, ask: string, answer: string) { add(id, clock() + 60_000, ask, answer); },
    };
  }
  const chat = () => screen.findByTestId(`chat-turn:${identity}:0`);
  const cards = () => screen.queryAllByTestId(`question-set:${PENDING_ID}`);
  const receipts = () => screen.queryAllByTestId(`question-set-receipt:${PENDING_ID}`);
  /** Answers the open set on its card in the chat. */
  const answerInChat = async () => {
    await chat();
    const card = await screen.findByTestId(`question-set:${PENDING_ID}`);
    await waitFor(() => expect((within(card).getByRole("radio", { name: "Europe" }) as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(within(card).getByRole("radio", { name: "Europe" }));
    return card;
  };
  const sendAnswers = (card: HTMLElement) => fireEvent.click(within(card).getByRole("button", { name: "Send answers" }));
  const laterTurn = async (gatewayOf: ReturnType<typeof chatGateway>) => {
    await act(async () => { gatewayOf.later("turn-2", "Go on with Europe.", "Europe goes first."); });
    return screen.findByTestId(`chat-turn:${identity}:1`);
  };

  it("sends a set's answers once", async () => {
    const gatewayOf = chatGateway();
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    const card = await answerInChat();
    // The open set is below the newest turn, and can be answered there.
    expect((await chat()).compareDocumentPosition(card) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    sendAnswers(card);
    sendAnswers(card);
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    await laterTurn(gatewayOf);
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 30)); });
    expect(gatewayOf.decided.length).toBe(1);
    expect(gatewayOf.decided[0]).toMatchObject({
      pending_id: PENDING_ID, presented_digest: DIGEST,
      answers: [{ question_id: "q-region", answer: { kind: "choice", choice_id: "c-eu" } }],
    });
  });

  it("does not draw the answered card again below a later turn", async () => {
    const gatewayOf = chatGateway();
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    sendAnswers(await answerInChat());
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    await laterTurn(gatewayOf);
    await waitFor(() => expect(cards().length).toBe(0));
    // Nothing of the set is asked again below the newest turn.
    expect(screen.queryByRole("button", { name: "Send answers" }) === null).toBe(true);
  });

  it("does not draw the answered card again when the person goes to another agent and back", async () => {
    const gatewayOf = chatGateway();
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    sendAnswers(await answerInChat());
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    fireEvent.click(await screen.findByTestId(`sidebar-agent:${other}`));
    await waitFor(() => expect(screen.queryByTestId(`chat-turn:${identity}:0`) === null).toBe(true));
    fireEvent.click(await screen.findByTestId(`sidebar-agent:${identity}`));
    await chat();
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 30)); });
    expect(cards().length).toBe(0);
  });

  it("does not draw the answered card again after the page is loaded again", async () => {
    const gatewayOf = chatGateway();
    const first = render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    sendAnswers(await answerInChat());
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    first.unmount();
    // The page again: the gateway lists no open set of the agent.
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    await chat();
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 30)); });
    expect(cards().length).toBe(0);
    expect(screen.queryByRole("button", { name: "Send answers" }) === null).toBe(true);
  });

  it("keeps the receipt with the turn it was answered after, above later turns", async () => {
    const gatewayOf = chatGateway();
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    sendAnswers(await answerInChat());
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    const second = await laterTurn(gatewayOf);
    await waitFor(() => expect(receipts().length).toBe(1));
    const receipt = receipts()[0];
    // Folded: one line. Its questions and the answers given are inside it.
    expect(receipt.tagName).toBe("DETAILS");
    expect((receipt as HTMLDetailsElement).open).toBe(false);
    expect(receipt.querySelector("summary")?.textContent).toContain("Answered");
    expect(receipt.querySelector("summary")?.textContent).toContain("Release plan");
    expect(receipt.textContent).toContain("Which region goes first?");
    expect(receipt.textContent).toContain("Europe");
    expect(receipt.textContent?.includes("United States")).toBe(false);
    // After the turn it was answered after, before the later one.
    expect((await chat()).compareDocumentPosition(receipt) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(receipt.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Nothing of it can be sent again.
    expect(within(receipt).queryByRole("radio") === null).toBe(true);
    expect(within(receipt).queryByRole("button", { name: "Send answers" }) === null).toBe(true);
  });

  it("shows an answered set once, as a read-only row of the question and the answer where it was answered, and never below the turns that follow", async () => {
    const gatewayOf = chatGateway();
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    // While it is open the set can be answered, below the newest turn.
    const card = await answerInChat();
    expect(within(card).getByRole("button", { name: "Send answers" }) !== null).toBe(true);
    expect(receipts().length).toBe(0);
    sendAnswers(card);
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    await waitFor(() => expect(receipts().length).toBe(1));
    const transcript = () => screen.getByLabelText("Conversation transcript");
    const count = (words: string) => (transcript().textContent ?? "").split(words).length - 1;
    const row = () => {
      const receipt = receipts()[0] as HTMLDetailsElement;
      return {
        open: receipt.open,
        asked: [...receipt.querySelectorAll("dt")].map((term) => term.textContent),
        answered: [...receipt.querySelectorAll("dd")].map((answer) => answer.textContent),
        controls: receipt.querySelectorAll("input, textarea, select, button, a[href], [contenteditable]").length,
      };
    };
    const answeredRow = { open: false, asked: ["Which region goes first?"], answered: ["Europe"], controls: 0 };
    expect(row()).toEqual(answeredRow);
    // Its line opens it to the question and the answer, and folds it again.
    const line = receipts()[0].querySelector("summary") as HTMLElement;
    fireEvent.click(line);
    expect(row()).toEqual({ ...answeredRow, open: true });
    fireEvent.click(line);
    expect(row()).toEqual(answeredRow);
    // Two later turns: still one row, where it was, and the question is
    // nowhere else in the chat; nothing of the set stands below them.
    await laterTurn(gatewayOf);
    await act(async () => { gatewayOf.add("turn-3", gatewayOf.clock() + 120_000, "And after Europe?", "The United States follow."); });
    const last = await screen.findByTestId(`chat-turn:${identity}:2`);
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 30)); });
    expect(cards().length).toBe(0);
    expect(receipts().length).toBe(1);
    expect(row()).toEqual(answeredRow);
    expect(count("Which region goes first?")).toBe(1);
    expect(screen.queryByRole("button", { name: "Send answers" }) === null).toBe(true);
    expect(screen.queryByRole("radio", { name: "Europe" }) === null).toBe(true);
    const receipt = receipts()[0];
    const first = await chat();
    const second = await screen.findByTestId(`chat-turn:${identity}:1`);
    expect(first.compareDocumentPosition(receipt) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(receipt.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(receipt.compareDocumentPosition(last) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Below the newest turn: no question set, open or answered.
    const below = [...transcript().querySelectorAll("[data-testid^='question-set']")]
      .filter((element) => Boolean(last.compareDocumentPosition(element) & Node.DOCUMENT_POSITION_FOLLOWING) && !last.contains(element));
    expect(below.map((element) => element.getAttribute("data-testid"))).toEqual([]);
  });

  it("keeps the receipt above the turn its answers start, which begins when the gateway decides", async () => {
    const gatewayOf = chatGateway();
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    sendAnswers(await answerInChat());
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    await waitFor(() => expect(receipts().length).toBe(1));
    // The gateway hands the answers to the agent at once: the turn they
    // start begins at the decision's own instant, which is not later than
    // the moment this page had the reply.
    await act(async () => { gatewayOf.add("turn-2", gatewayOf.decidedAt[0], "The answers.", "Europe goes first."); });
    const second = await screen.findByTestId(`chat-turn:${identity}:1`);
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 30)); });
    expect(receipts().length).toBe(1);
    const receipt = receipts()[0];
    expect((await chat()).compareDocumentPosition(receipt) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(receipt.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps the receipt after a turn that had begun when the gateway decided, also when the gateway's clock is ahead of this page's", async () => {
    const gatewayOf = chatGateway(600_000);
    // A second turn began a moment ago, by the gateway's clock.
    gatewayOf.add("turn-2", gatewayOf.clock() - 5_000, "Start with the plan.", "Which region, then?");
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    sendAnswers(await answerInChat());
    await waitFor(() => expect(gatewayOf.decided.length).toBe(1));
    await waitFor(() => expect(receipts().length).toBe(1));
    const second = await screen.findByTestId(`chat-turn:${identity}:1`);
    // After the newest turn: it had begun when the set was answered.
    expect(second.compareDocumentPosition(receipts()[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it("keeps the focus on the answered set's own line in the chat while the line of another set that left is on screen", async () => {
    const SECOND_ID = `decision:${"e".repeat(64)}`;
    const gatewayOf = chatGateway(0, [{
      ...questionSet(), pending_id: SECOND_ID, title: "Rollout order", subject_digest: `sha256:${"f".repeat(64)}`,
      questions: [{ id: "r-order", prompt: "Which service goes first?", choices: [{ id: "r-api", label: "The API" }, { id: "r-web", label: "The site" }] }],
    }]);
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    const card = await answerInChat();
    await screen.findByTestId(`question-set:${SECOND_ID}`);
    // The second set is resolved elsewhere; the page reads the sets again
    // when its tab comes back, and shows the one line for the card that left.
    gatewayOf.drop(SECOND_ID);
    let visibility = "hidden";
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
    try {
      await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
      visibility = "visible";
      await act(async () => { document.dispatchEvent(new Event("visibilitychange")); });
      await waitFor(() => expect(screen.queryAllByTestId(`question-set:${SECOND_ID}`).length).toBe(0));
    } finally {
      delete (document as unknown as { visibilityState?: string }).visibilityState;
    }
    expect((await screen.findByTestId("question-set-notice")).textContent).toBe("This question is no longer listed here.");
    // The first set is answered with the focus in its card.
    const sendButton = within(card).getByRole("button", { name: "Send answers" });
    sendButton.focus();
    fireEvent.click(sendButton);
    await waitFor(() => expect(receipts().length).toBe(1));
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 30)); });
    // The receipt stands with the turn, the line with the conversation:
    // the answered set's own line has the focus.
    expect(screen.queryAllByTestId("question-set-notice").length).toBe(1);
    expect(document.activeElement?.tagName).toBe("SUMMARY");
    expect(document.activeElement?.parentElement?.getAttribute("data-testid")).toBe(`question-set-receipt:${PENDING_ID}`);
  });
});
