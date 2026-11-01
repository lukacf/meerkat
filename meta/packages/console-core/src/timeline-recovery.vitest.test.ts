import { describe, expect, it } from "vitest";

import { createMetaConsoleController as coreController, type MetaConsoleTransport } from "./headless";
import { createMetaConsoleController as consoleController } from "../../../console/src/lib/headless";
import type { ConsoleFrame, ConsoleTimelinePage } from "./runtime-types";
import {
  consoleFrameRowKey,
  createConsoleTimelineDedupe,
  insertConsoleFrame,
  subscribeTimelineWithRecovery,
} from "./timeline-recovery";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function frame(id: string, extra: Partial<ConsoleFrame> = {}): ConsoleFrame {
  return { id, cursor: `console:${id}`, event: "interaction_complete", identity: "domain:billing", timestampMs: 1, data: {}, ...extra };
}

function page(...frames: ConsoleFrame[]): ConsoleTimelinePage {
  return { frames, latestCursor: frames.at(-1)?.cursor, available: true };
}

/** A transport whose recent page is `pages[n]` for the n-th query, recording what each call received. */
function fakeTransport(pages: ConsoleTimelinePage[]) {
  const queryInputs: Array<Record<string, unknown>> = [];
  const streamInputs: Array<Record<string, unknown>> = [];
  const transport = {
    loadExperience: async () => ({}),
    capabilities: async () => ({ methods: [] }),
    send: async () => ({}),
    async queryTimeline(input: Record<string, unknown>) {
      queryInputs.push(input);
      return pages[Math.min(queryInputs.length - 1, pages.length - 1)]!;
    },
    subscribeTimeline(input: Record<string, unknown>) {
      streamInputs.push(input);
      return () => {};
    },
  } as unknown as MetaConsoleTransport;
  return { transport, queryInputs, streamInputs };
}

describe("one timeline dedupe per scope", () => {
  it("a re-subscription with the scope's dedupe delivers only frames not accepted before", async () => {
    const { transport } = fakeTransport([page(frame("a"), frame("b")), page(frame("a"), frame("b"), frame("c"))]);
    const dedupe = createConsoleTimelineDedupe();
    const seen: string[] = [];
    const first = await subscribeTimelineWithRecovery(transport, { limit: 200, dedupe }, (f) => seen.push(f.id));
    first();
    // The console subscribes again when its storage namespace changes.
    const second = await subscribeTimelineWithRecovery(transport, { limit: 200, dedupe }, (f) => seen.push(f.id));
    second();
    expect(seen).toEqual(["a", "b", "c"]);
  });

  it("without a dedupe each subscription starts with none (the cause of the duplicate rows)", async () => {
    const { transport } = fakeTransport([page(frame("a"), frame("b"))]);
    const seen: string[] = [];
    (await subscribeTimelineWithRecovery(transport, { limit: 200 }, (f) => seen.push(f.id)))();
    (await subscribeTimelineWithRecovery(transport, { limit: 200 }, (f) => seen.push(f.id)))();
    expect(seen).toEqual(["a", "b", "a", "b"]);
  });

  it("delivers a newer version of an accepted frame", async () => {
    const { transport } = fakeTransport([page(frame("a", { frameVersion: 1 })), page(frame("a", { frameVersion: 2 }))]);
    const dedupe = createConsoleTimelineDedupe();
    const versions: Array<number | undefined> = [];
    (await subscribeTimelineWithRecovery(transport, { dedupe }, (f) => versions.push(f.frameVersion)))();
    (await subscribeTimelineWithRecovery(transport, { dedupe }, (f) => versions.push(f.frameVersion)))();
    expect(versions).toEqual([1, 2]);
  });

  it("never hands the dedupe to the transport", async () => {
    const { transport, queryInputs, streamInputs } = fakeTransport([page(frame("a"))]);
    (await subscribeTimelineWithRecovery(transport, { limit: 5, dedupe: createConsoleTimelineDedupe() }, () => {}))();
    expect(queryInputs).toHaveLength(1);
    expect(streamInputs).toHaveLength(1);
    expect("dedupe" in queryInputs[0]!).toBe(false);
    expect("dedupe" in streamInputs[0]!).toBe(false);
    expect(queryInputs[0]!.limit).toBe(5);
  });

  it.each([["core", coreController], ["console", consoleController]] as const)(
    "the %s controller passes the scope's dedupe through subscribeWithBackfill",
    async (_name, create) => {
      const { transport } = fakeTransport([page(frame("a"), frame("b"))]);
      const controller = create({ transport });
      const dedupe = createConsoleTimelineDedupe();
      const seen: string[] = [];
      const input = { limit: 200, dedupe };
      (await controller.timeline.subscribeWithBackfill(input, (fact) => seen.push(fact.value.id)))();
      (await controller.timeline.subscribeWithBackfill(input, (fact) => seen.push(fact.value.id)))();
      await tick();
      expect(seen).toEqual(["a", "b"]);
    },
  );

  it("keeps a bounded number of keys, the oldest going first", () => {
    const dedupe = createConsoleTimelineDedupe(2);
    dedupe.add("a");
    dedupe.add("b");
    dedupe.add("c");
    expect(dedupe.size).toBe(2);
    expect(dedupe.has("a")).toBe(false);
    expect(dedupe.has("c")).toBe(true);
  });
});

describe("frame buffers hold one copy of a frame", () => {
  it("ignores a copy of a held frame and lists new frames first", () => {
    let buffer: ConsoleFrame[] = [];
    for (const f of [frame("a"), frame("b"), { ...frame("a") }]) buffer = insertConsoleFrame(buffer, f, 10) ?? buffer;
    expect(buffer.map((f) => f.id)).toEqual(["b", "a"]);
    expect(insertConsoleFrame(buffer, frame("b"), 10)).toBeNull();
  });

  it("replaces a held frame in place with a newer version only", () => {
    const buffer = [frame("b"), frame("a", { frameVersion: 2, status: "queued" })];
    expect(insertConsoleFrame(buffer, frame("a", { frameVersion: 1, status: "old" }), 10)).toBeNull();
    const newer = insertConsoleFrame(buffer, frame("a", { frameVersion: 3, status: "completed" }), 10)!;
    expect(newer.map((f) => `${f.id}:${f.frameVersion ?? 0}:${f.status ?? ""}`)).toEqual(["b:0:", "a:3:completed"]);
    const later = insertConsoleFrame(newer, frame("a", { frameVersion: 3, status: "final", updatedAtMs: 9 }), 10)!;
    expect(later[1]!.status).toBe("final");
    // The held buffer is never mutated.
    expect(buffer[1]!.status).toBe("queued");
  });

  it("keeps an update envelope that shares its frame's id apart from that frame", () => {
    const held = [frame("a", { event: "user_input", frameVersion: 1 })];
    const envelope = frame("a", { event: "frame_updated", frameVersion: 2 });
    expect(insertConsoleFrame(held, envelope, 10)!.map((f) => f.event)).toEqual(["frame_updated", "user_input"]);
  });

  it("always adds a frame without an id and keeps at most `cap` frames", () => {
    let buffer: ConsoleFrame[] = [];
    for (let i = 0; i < 5; i++) buffer = insertConsoleFrame(buffer, frame("", { timestampMs: i }), 3) ?? buffer;
    expect(buffer.map((f) => f.timestampMs)).toEqual([4, 3, 2]);
  });

  it("keys a row by id and version, and an id-less frame by its own object", () => {
    expect(consoleFrameRowKey(frame("a"))).toBe("a:0");
    expect(consoleFrameRowKey(frame("a", { frameVersion: 4 }))).toBe("a:4");
    const local = frame("");
    const other = frame("");
    expect(consoleFrameRowKey(local)).toBe(consoleFrameRowKey(local));
    expect(consoleFrameRowKey(local)).not.toBe(consoleFrameRowKey(other));
  });
});
