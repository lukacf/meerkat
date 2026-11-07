import { describe, expect, it, vi } from "vitest";

import {
  CONSOLE_ATTENTION_LIMITS,
  consoleAttentionUnavailable,
  normalizeConsoleAttention,
  privateReplyText,
  startConsoleAttentionPolling,
  type ConsoleAttentionPollingEnvironment,
  type ConsoleAttentionSnapshot,
} from "./console-attention";

function wire(id: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id,
    kind: "private_reply",
    created_at_ms: 1_000,
    text: `Reply ${id}`,
    origin: { identity: "alpha", interaction_id: `interaction-${id}`, input_frame_id: `frame-${id}` },
    ...extra,
  };
}

describe("normalizeConsoleAttention", () => {
  it("reads private replies and skips what it cannot show", () => {
    const items = normalizeConsoleAttention({
      items: [
        wire("a"),
        wire("b", { kind: "a_future_kind" }),
        wire("c", { text: "  " }),
        wire("d", { origin: { identity: "alpha", interaction_id: "x" } }),
        wire("e", { created_at_ms: "yesterday" }),
        wire("a", { text: "A duplicate id" }),
        null,
        "item",
      ],
    });
    expect(items).toEqual([{
      id: "a",
      kind: "private_reply",
      createdAtMs: 1_000,
      text: "Reply a",
      redacted: false,
      origin: { identity: "alpha", interactionId: "interaction-a", inputFrameId: "frame-a" },
    }]);
  });

  it("keeps a reply a policy rewrote, and one it left no text in, never dropping it", () => {
    const { text: _text, ...withoutText } = wire("hidden", { redacted: true });
    const items = normalizeConsoleAttention({
      items: [
        withoutText,
        wire("rewritten", { text: "[withheld]", redacted: true }),
        wire("blank", { text: "  ", redacted: true }),
      ],
    });
    expect(items?.map((item) => [item.id, item.text, item.redacted])).toEqual([
      ["hidden", null, true],
      ["rewritten", "[withheld]", true],
      ["blank", null, true],
    ]);
  });

  it("bounds the list and each text", () => {
    const many = Array.from({ length: CONSOLE_ATTENTION_LIMITS.items + 5 }, (_, index) => wire(`item-${index}`));
    expect(normalizeConsoleAttention({ items: many })).toHaveLength(CONSOLE_ATTENTION_LIMITS.items);
    const [long] = normalizeConsoleAttention({ items: [wire("long", { text: "x".repeat(CONSOLE_ATTENTION_LIMITS.textLength + 10) })] }) ?? [];
    expect(long.text).toHaveLength(CONSOLE_ATTENTION_LIMITS.textLength);
    expect(long.text.endsWith("…")).toBe(true);
  });

  it("answers null for an answer without an item list", () => {
    expect(normalizeConsoleAttention(null)).toBeNull();
    expect(normalizeConsoleAttention({ pending: [] })).toBeNull();
    expect(normalizeConsoleAttention({ items: [] })).toEqual([]);
  });
});

describe("consoleAttentionUnavailable", () => {
  it("is a missing capability or an unknown method, nothing else", () => {
    expect(consoleAttentionUnavailable({ kind: "console-capability-unavailable" })).toBe(true);
    expect(consoleAttentionUnavailable({ rpcError: { code: -32601 } })).toBe(true);
    expect(consoleAttentionUnavailable({ rpcError: { code: -32000 } })).toBe(false);
    expect(consoleAttentionUnavailable(new Error("network"))).toBe(false);
    expect(consoleAttentionUnavailable(null)).toBe(false);
  });
});

describe("privateReplyText", () => {
  it("is the text of a private reply frame only", () => {
    expect(privateReplyText({ event: "private_reply", data: { text: "Done." } })).toBe("Done.");
    expect(privateReplyText({ event: "private_reply", data: { text: " " } })).toBeNull();
    expect(privateReplyText({ event: "private_reply", data: null })).toBeNull();
    expect(privateReplyText({ event: "text_complete", data: { text: "Done." } })).toBeNull();
  });
});

function environment(visible = true) {
  const intervals: Array<() => void> = [];
  const visibleListeners: Array<() => void> = [];
  const env: ConsoleAttentionPollingEnvironment & { visibleNow: boolean } = {
    visibleNow: visible,
    visible: () => env.visibleNow,
    onVisible: (callback) => {
      visibleListeners.push(callback);
      return () => visibleListeners.splice(visibleListeners.indexOf(callback), 1);
    },
    setInterval: (callback) => {
      intervals.push(callback);
      return intervals.length - 1;
    },
    clearInterval: (handle) => {
      intervals[handle as number] = () => {};
    },
  };
  return {
    env,
    tick: () => intervals.forEach((callback) => callback()),
    becomeVisible: () => [...visibleListeners].forEach((callback) => callback()),
    listeners: () => visibleListeners.length,
  };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve();
}

describe("startConsoleAttentionPolling", () => {
  it("publishes each completed read, never one a newer read superseded", async () => {
    const { env, tick } = environment();
    const answers: Array<(value: unknown) => void> = [];
    const read = vi.fn(() => new Promise<unknown>((resolve) => answers.push(resolve)));
    const published: ConsoleAttentionSnapshot[] = [];
    const polling = startConsoleAttentionPolling(read, (snapshot) => published.push(snapshot), env);
    tick();
    expect(read).toHaveBeenCalledTimes(2);
    answers[0]({ items: [wire("old")] });
    answers[1]({ items: [wire("new")] });
    await flush();
    expect(published.map((snapshot) => snapshot.items.map((item) => item.id))).toEqual([["new"]]);
    polling.stop();
  });

  it("stops for good on a gateway without an attention list", async () => {
    const { env, tick, becomeVisible, listeners } = environment();
    const read = vi.fn(async () => {
      throw { kind: "console-capability-unavailable" };
    });
    const published: ConsoleAttentionSnapshot[] = [];
    startConsoleAttentionPolling(read, (snapshot) => published.push(snapshot), env);
    await flush();
    expect(published).toEqual([{ status: "unavailable", items: [] }]);
    tick();
    becomeVisible();
    await flush();
    expect(read).toHaveBeenCalledTimes(1);
    expect(listeners()).toBe(0);
  });

  it("keeps the last items on any other failure", async () => {
    const { env, tick } = environment();
    let fail = false;
    const read = vi.fn(async () => {
      if (fail) throw new Error("network");
      return { items: [wire("a")] };
    });
    const published: ConsoleAttentionSnapshot[] = [];
    const polling = startConsoleAttentionPolling(read, (snapshot) => published.push(snapshot), env);
    await flush();
    fail = true;
    tick();
    await flush();
    expect(read).toHaveBeenCalledTimes(2);
    expect(published).toHaveLength(1);
    polling.stop();
  });

  it("reads only while visible, again when visible, and on refresh", async () => {
    const { env, tick, becomeVisible } = environment(false);
    const read = vi.fn(async () => ({ items: [] }));
    const polling = startConsoleAttentionPolling(read, () => {}, env);
    tick();
    expect(read).not.toHaveBeenCalled();
    polling.refresh();
    expect(read).toHaveBeenCalledTimes(1);
    env.visibleNow = true;
    becomeVisible();
    expect(read).toHaveBeenCalledTimes(2);
    polling.stop();
    tick();
    polling.refresh();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it("aborts the read in flight when stopped and publishes nothing after", async () => {
    const { env } = environment();
    let signal: AbortSignal | undefined;
    let answer: (value: unknown) => void = () => {};
    const read = vi.fn((given: AbortSignal) => {
      signal = given;
      return new Promise<unknown>((resolve) => { answer = resolve; });
    });
    const publish = vi.fn();
    const polling = startConsoleAttentionPolling(read, publish, env);
    polling.stop();
    expect(signal?.aborted).toBe(true);
    answer({ items: [wire("late")] });
    await flush();
    expect(publish).not.toHaveBeenCalled();
  });
});
