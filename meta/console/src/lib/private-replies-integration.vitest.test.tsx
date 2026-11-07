/**
 * Private replies in the stock console: a reply meant for the signed-in
 * person alone (`meta/console/attention`) is counted in "Needs you", listed in
 * the inbox with the conversation it answers one click away, and dismissed
 * there. Items belong to the scope (person and runtime) that read them: a
 * user switch hides the previous person's items at once, and a read that
 * started before the switch never shows them to the next person.
 */
import React from "react";
import { act, fireEvent, render, waitFor, within, type RenderResult } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ConsoleApp } from "../ConsoleApp";
import { CONSOLE_RPC_METHODS } from "./contract";
import type { MetaConsoleTransport } from "./headless";

const ATTENTION_METHODS = ["meta/console/attention", "meta/console/attention/dismiss"];
const ALICE = "scope-alice";
const BOB = "scope-bob";
const AGENT = "alpha";
const REPLY = "Your release is approved.";

type Row = Record<string, unknown>;

function replyItem(id: string, text: string): Row {
  return {
    id,
    kind: "private_reply",
    created_at_ms: Date.now() - 60_000,
    text,
    origin: { identity: AGENT, interaction_id: `interaction-${id}`, input_frame_id: `frame-${id}` },
  };
}

interface Harness extends MetaConsoleTransport {
  /** The signed-in person: whose experience and items the gateway answers. */
  scope: string;
  readOnly: boolean;
  /** Whether the gateway serves the attention list. */
  attention: boolean;
  items: Record<string, Row[]>;
  /** Attention reads wait for the test while set. */
  holdReads: boolean;
  held: Array<() => void>;
  commands: Array<{ command: string; params: Record<string, unknown> }>;
}

function transport(): Harness {
  const harness: Harness = {
    scope: ALICE,
    readOnly: false,
    attention: true,
    items: { [ALICE]: [replyItem("reply-1", REPLY)] },
    holdReads: false,
    held: [],
    commands: [],
    loadExperience: async () => ({
      contract_version: "test",
      runtime_id: "replies-runtime",
      storage_scope: harness.scope,
      console_config: {},
      console_policy: harness.readOnly ? { read_only: true } : {},
      agent_sidebar: {
        live_snapshot: {
          agents: [{
            identity: AGENT,
            member_id: AGENT,
            agent_id: AGENT,
            label: "Alpha",
            kind: "mob_agent",
            role: "worker",
            state: "active",
            addressable: true,
            affordances: { can_send_message: true },
          }],
        },
      },
      activity_feed: { filter_presets: [], active_preset_id: "all" },
    }) as never,
    loadModules: async () => ({ modules: [] }) as never,
    capabilities: async () => ({
      version: "test",
      methods: [...new Set([...Object.values(CONSOLE_RPC_METHODS), ...ATTENTION_METHODS])]
        .filter((method) => harness.attention || !ATTENTION_METHODS.includes(method)),
    }) as never,
    queryTimeline: async () => ({ frames: [], available: true }) as never,
    subscribeTimeline: () => () => undefined,
    send: async (input) => ({ interaction_id: "sent", identity: input.identity, cursor: "console:x" }) as never,
    executeCommand: async (input) => {
      const params = (input.params || {}) as Record<string, unknown>;
      harness.commands.push({ command: input.command, params });
      if (input.command === "listConsoleAttention") {
        // The gateway answers the caller's own items, as of the request.
        const items = [...(harness.items[harness.scope] ?? [])];
        const answer = { command: input.command, accepted: true, result: { items, retention: "durable" } };
        if (!harness.holdReads) return answer as never;
        return new Promise((resolve) => {
          harness.held.push(() => resolve(answer as never));
        });
      }
      if (input.command === "dismissConsoleAttention") {
        const items = harness.items[harness.scope] ?? [];
        const dismissed = items.some((item) => item.id === params.item_id);
        harness.items[harness.scope] = items.filter((item) => item.id !== params.item_id);
        return { command: input.command, accepted: true, result: { item_id: params.item_id, dismissed } } as never;
      }
      return { command: input.command, accepted: true, result: { pending: [] } } as never;
    },
  };
  return harness;
}

async function settle(ms = 300): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

function needsYou(view: RenderResult): HTMLElement {
  return within(view.getByTestId("approval-attention")).getByRole("button");
}

async function openInbox(view: RenderResult): Promise<void> {
  await act(async () => {
    fireEvent.click(needsYou(view));
  });
  await settle();
}

function attentionReads(harness: Harness): number {
  return harness.commands.filter((entry) => entry.command === "listConsoleAttention").length;
}

describe("private replies in the console", () => {
  beforeEach(() => window.localStorage.clear());
  afterEach(() => window.localStorage.clear());

  it("private_reply_shown_to_addressee_only: the person's reply is theirs, and a user switch hides it at once", async () => {
    const harness = transport();
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
    try {
      const view = render(<ConsoleApp baseUrl="" transport={harness} />);
      await settle();
      expect(needsYou(view).getAttribute("aria-label")).toContain("1 reply for you");
      await openInbox(view);
      const card = view.getByTestId("console-attention:reply-1");
      expect(within(card).getByText(REPLY)).toBeTruthy();

      // A read for Alice is in flight when Bob signs in on the same browser.
      harness.holdReads = true;
      await act(async () => {
        document.dispatchEvent(new Event("visibilitychange"));
      });
      await settle();
      const aliceReads = attentionReads(harness);
      harness.scope = BOB;
      // The periodic experience reload sees Bob's scope.
      await act(async () => {
        vi.advanceTimersByTime(15_000);
      });
      await settle();
      expect(attentionReads(harness)).toBeGreaterThan(aliceReads);
      expect(view.queryByText(REPLY)).toBeNull();
      expect(needsYou(view).getAttribute("aria-label")).not.toContain("for you");

      // Every held read answers now: Alice's late answer is not Bob's.
      harness.items[BOB] = [];
      await act(async () => {
        for (const release of harness.held.splice(0)) release();
      });
      await settle();
      expect(view.queryByText(REPLY)).toBeNull();
      expect(needsYou(view).getAttribute("aria-label")).not.toContain("for you");
      await openInbox(view);
      expect(view.queryByTestId("console-attention:reply-1")).toBeNull();
      expect(view.queryByText(REPLY)).toBeNull();
      view.unmount();
    } finally {
      vi.useRealTimers();
    }
  }, 30_000);

  it("a reply opens the chat it answers", async () => {
    const harness = transport();
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await settle();
    await openInbox(view);
    await act(async () => {
      fireEvent.click(view.getByTestId("console-attention-open:reply-1"));
    });
    await view.findByTestId(`chat-pane:${AGENT}`, undefined, { timeout: 5_000 });
    view.unmount();
  }, 30_000);

  it("a reply is dismissed from the inbox", async () => {
    const harness = transport();
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await settle();
    await openInbox(view);
    await act(async () => {
      fireEvent.click(view.getByTestId("console-attention-dismiss:reply-1"));
    });
    await waitFor(() => expect(view.queryByTestId("console-attention:reply-1")).toBeNull());
    expect(harness.commands.find((entry) => entry.command === "dismissConsoleAttention")?.params)
      .toEqual({ item_id: "reply-1" });
    expect(needsYou(view).getAttribute("aria-label")).not.toContain("for you");
    view.unmount();
  }, 30_000);

  it("a reply whose text a policy removed is counted, says its content is hidden by policy, and is dismissed", async () => {
    const harness = transport();
    const { text: _text, ...hidden } = { ...replyItem("reply-hidden", REPLY), redacted: true };
    harness.items[ALICE] = [hidden];
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await settle();
    expect(needsYou(view).getAttribute("aria-label")).toContain("1 reply for you");
    await openInbox(view);
    const card = view.getByTestId("console-attention:reply-hidden");
    expect(within(card).getByText("Content hidden by policy")).toBeTruthy();
    await act(async () => {
      fireEvent.click(view.getByTestId("console-attention-dismiss:reply-hidden"));
    });
    await waitFor(() => expect(view.queryByTestId("console-attention:reply-hidden")).toBeNull());
    expect(harness.commands.find((entry) => entry.command === "dismissConsoleAttention")?.params)
      .toEqual({ item_id: "reply-hidden" });
    view.unmount();
  }, 30_000);

  it("a read-only console shows the reply and offers no Dismiss", async () => {
    const harness = transport();
    harness.readOnly = true;
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await settle();
    await openInbox(view);
    const card = view.getByTestId("console-attention:reply-1");
    expect(within(card).getByText(REPLY)).toBeTruthy();
    expect(view.queryByTestId("console-attention-dismiss:reply-1")).toBeNull();
    view.unmount();
  }, 30_000);

  it("a gateway without an attention list is not asked for one", async () => {
    const harness = transport();
    harness.attention = false;
    const view = render(<ConsoleApp baseUrl="" transport={harness} />);
    await settle();
    await act(async () => {
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await settle();
    expect(attentionReads(harness)).toBe(0);
    expect(needsYou(view).getAttribute("aria-label")).not.toContain("for you");
    await openInbox(view);
    expect(view.queryByTestId("console-attention-list")).toBeNull();
    view.unmount();
  }, 30_000);
});
