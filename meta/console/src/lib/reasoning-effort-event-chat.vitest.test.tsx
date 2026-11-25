import React from "react";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { conversationEntryText } from "@console-core";
import { ConsoleApp } from "../ConsoleApp";
import type { ConsoleWhoami, MetaConsoleTransport } from "./headless";
import type { ConsoleFrame } from "../types";
import { mapFramesToTimelineEntries as stock } from "./adapters";
import { mapFramesToTimelineEntries as shared } from "../../../packages/console-core/src/adapters";

// The runtime reports, for each request it hands a provider, the reasoning
// effort the request was sent with (`request_reasoning_effort_resolved`).
// That is a record for whatever counts or lists events, and never a line of
// a chat. The filter and its own rows (replay, live updates, the raw log and
// the activity rail) are in adapters.ts and adapters.test.ts. These rows add
// what those do not hold: frames that name where they came from (the
// gateway's kept events, a session's history), the two kinds of line that
// must stay beside it (another runtime event, a peer's message), and the
// whole Console drawing a chat on load and from its open stream.

const EFFORT_EVENT = "request_reasoning_effort_resolved";
const identity = "identity:release-planner";
const options = { renderInteractionStartsAsUser: true, renderTextDeltas: true, textMode: "markdown" as const };

function frame(id: string, event: string, data: unknown, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, event, data, timestampMs: 1_000, identity, ...extra };
}

const effortPayload = (turn: number) => ({
  turn_number: turn,
  resolution: {
    requested: "high", level: "high", provider: "openai", model: "gpt-5.5",
    route: "responses", fields: ["reasoning.effort"], facts: "catalog", decided_by: "request",
  },
});

/** The event as the gateway's stream carries it during a run. */
const effort = (id: string, atMs: number, extra: Partial<ConsoleFrame> = {}) =>
  frame(id, EFFORT_EVENT, effortPayload(1), { timestampMs: atMs, runId: "run-1", interactionId: "turn-1", sourceKind: "console_event", ...extra });

/** Another runtime event the Console has no renderer of its own for. */
const otherEvent = (atMs: number) =>
  frame("skipped", "requested_compaction_skipped", { reason: "Nothing to compact yet" }, { timestampMs: atMs, sourceKind: "console_event" });

/** A peer's message, as the typed notice the runtime writes for it. */
const peerNotice = (atMs: number, extra: Partial<ConsoleFrame> = {}) =>
  frame("peer-notice", "system_notice", { message: {
    role: "system_notice", kind: "comms", body: "Peer message", blocks: [{
      type: "comms", direction: "incoming", kind: "message",
      peer: { id: "reviewer-peer", display_name: "release/reviewer" },
      content: [{ type: "text", text: "The plan reads well." }],
    }],
  } }, { timestampMs: atMs, ...extra });

type Entries = ReturnType<typeof stock>;
const effortLines = (entries: Entries) => entries.filter((entry) =>
  (entry.kind === "message" && entry.runtimeEvent?.eventType === EFFORT_EVENT)
  || /reasoning effort/iu.test(conversationEntryText(entry)));
const runtimeEventLines = (entries: Entries, eventType: string) => entries.filter((entry) =>
  entry.kind === "message" && entry.runtimeEvent?.eventType === eventType);
const peerBlocks = (entries: Entries) => entries
  .flatMap((entry) => (entry.kind === "message" ? entry.blocks || [] : []))
  .filter((block) => block.type === "tool-call" && block.peerIncoming);
const assistantTexts = (entries: Entries) => entries
  .filter((entry) => entry.identity.role === "assistant")
  .map((entry) => conversationEntryText(entry));

/** A settled turn, as a page that loads reads it: the gateway's kept event
 * frames and the session's own history rows. */
const reloadedFrames = () => [
  frame("ask", "user_input", { content: "Plan the release." }, { timestampMs: 1_000, interactionId: "turn-1", sourceKind: "send" }),
  effort("effort-kept", 1_150),
  effort("effort-history", 1_160, { sourceKind: "session_history" }),
  frame("done", "interaction_complete", { result: "Looking at it." }, { timestampMs: 1_300, runId: "run-1", interactionId: "turn-1", sourceKind: "console_event" }),
  otherEvent(1_400),
  peerNotice(1_500, { sourceKind: "session_history" }),
];

for (const [name, mapper] of [["stock", stock], ["shared", shared]] as const) {
  describe(`${name}: the reasoning effort a request was sent with is not a chat line`, () => {
    it("reconstructed: a transcript built again from kept event frames and history rows has no line of it", () => {
      const entries = mapper(null, reloadedFrames(), options);
      expect(effortLines(entries).map((entry) => conversationEntryText(entry))).toEqual([]);
      expect(assistantTexts(entries)).toEqual(["Looking at it."]);
    });

    it("control: another runtime event and a peer's message keep their lines beside it", () => {
      const entries = mapper(null, reloadedFrames(), options);
      expect(runtimeEventLines(entries, "requested_compaction_skipped").map((entry) => conversationEntryText(entry)))
        .toEqual(["Requested compaction skipped: Nothing to compact yet"]);
      const peers = peerBlocks(entries);
      expect(peers.length).toBe(1);
      expect(peers[0].peerBody).toBe("The plan reads well.");
    });
  });
}

describe("the reasoning effort event in the Console's chat", () => {
  const person: ConsoleWhoami = { actorKind: "person", principal: "a@example.test", approverIds: [], canAdminister: false, callerScope: "caller-a" };

  /** A gateway with one agent, the frames it kept, and an open stream. */
  function gateway(kept: ConsoleFrame[]) {
    const listeners = new Set<(frame: never) => void>();
    const fake: MetaConsoleTransport = {
      loadExperience: async () => ({
        runtime_id: "effort-event-test", storage_scope: "console-storage:v1:effort-event",
        console_config: { layout: { initial_agent: identity } },
        agent_sidebar: { live_snapshot: { agents: [{
          identity, member_id: identity, agent_id: identity, label: "Release Planner", kind: "member", state: "running",
          addressable: true, affordances: { can_send_message: true }, model_capabilities: { image_input: false },
        }] } },
        activity_feed: { filter_presets: [], active_preset_id: "all" },
      }) as never,
      loadModules: async () => ({ modules: [] }) as never,
      capabilities: async () => ({ version: "test", methods: ["meta/console/send", "meta/console/timeline"] }) as never,
      whoami: async () => person,
      queryTimeline: async () => ({ frames: kept as never, available: true }),
      subscribeTimeline: (_input, onFrame) => {
        listeners.add(onFrame as never);
        return () => { listeners.delete(onFrame as never); };
      },
      send: vi.fn(),
      executeCommand: async (input) => ({ command: input.command, accepted: true, result: {} }) as never,
    };
    return {
      fake,
      /** Frames that arrive on the open stream; the gateway keeps them too. */
      stream(frames: ConsoleFrame[]) {
        kept.push(...frames);
        for (const next of frames) for (const listener of listeners) listener(next as never);
      },
    };
  }
  /** The words of the chat's transcript: the turns and every line in them. */
  const chatText = async () => {
    await screen.findByTestId(`chat-turn:${identity}:0`);
    return screen.getByLabelText("Conversation transcript").textContent ?? "";
  };

  beforeEach(() => {
    vi.stubGlobal("localStorage", window.localStorage);
    window.localStorage.clear();
    window.sessionStorage.clear();
  });
  afterEach(async () => {
    await act(async () => { await new Promise((resolve) => window.setTimeout(resolve, 20)); });
    cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals();
  });

  it("reconstructed: a page that loads draws the turn, the other event and the peer's message, and no line of the event", async () => {
    render(<ConsoleApp baseUrl="" transport={gateway(reloadedFrames()).fake} />);
    await waitFor(async () => expect(await chatText()).toContain("Looking at it."));
    await waitFor(async () => expect(await chatText()).toContain("Requested compaction skipped"));
    const text = await chatText();
    expect(/reasoning effort/iu.test(text)).toBe(false);
    expect(text.includes(EFFORT_EVENT)).toBe(false);
  });

  it("live: the event on the open stream draws no line, and the other event after it draws its own", async () => {
    const t = Date.now() - 600_000;
    const gatewayOf = gateway([
      frame("ask", "user_input", { content: "Plan the release." }, { timestampMs: t, interactionId: "turn-1", sourceKind: "send" }),
      frame("done", "interaction_complete", { result: "Looking at it." }, { timestampMs: t + 300, runId: "run-1", interactionId: "turn-1", sourceKind: "console_event" }),
    ]);
    render(<ConsoleApp baseUrl="" transport={gatewayOf.fake} />);
    await waitFor(async () => expect(await chatText()).toContain("Looking at it."));
    await act(async () => { gatewayOf.stream([effort("effort-stream", t + 350), otherEvent(t + 400)]); });
    await waitFor(async () => expect(await chatText()).toContain("Requested compaction skipped"));
    const text = await chatText();
    expect(/reasoning effort/iu.test(text)).toBe(false);
    expect(text.includes(EFFORT_EVENT)).toBe(false);
  });
});
