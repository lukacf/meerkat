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
  let sets: unknown[] = [questionSet()];
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
      if (input.command === "listQuestionSets") return { command: input.command, accepted: true, result: { question_sets: sets } } as never;
      if (input.command === "decideGating") {
        const params = { ...(input.params as Record<string, unknown>) };
        decided.push(params);
        return { command: input.command, accepted: true, result: decide(params) } as never;
      }
      return { command: input.command, accepted: true, result: {} } as never;
    },
  };
  return { fake, decided, list(next: unknown[]) { sets = next; } };
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
