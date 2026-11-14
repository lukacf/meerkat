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
});
