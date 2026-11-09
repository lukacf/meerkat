import assert from "node:assert/strict";
import test from "node:test";
import { createMetaConsoleController as sharedController, type MetaConsoleTransport, type ConsoleTimelineSubscribeInput } from "./headless";
import { createMetaConsoleController as stockController } from "../../../console/src/lib/headless";
import type { ConsoleFrame, ConsoleTimelinePage } from "./runtime-types";
import type { ConsoleTransportState } from "./timeline-subscription";
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const frame = (n: number): ConsoleFrame => ({ id: `frame:${n}`, cursor: `console:${n}`, event: "text_delta", data: String(n) });
const page = (n: number): ConsoleTimelinePage => ({ frames: [frame(n)], latestCursor: `console:${n}`, available: true });
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (reason: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; }
function fixture(queries: Array<Promise<ConsoleTimelinePage> | ConsoleTimelinePage>) {
  const subscriptions: Array<{ input: ConsoleTimelineSubscribeInput; deliver(frame: ConsoleFrame): void; stopped: boolean }> = [];
  let queryCount = 0;
  const signals: Array<AbortSignal | undefined> = [];
  const transport = {
    loadExperience: async () => ({}), capabilities: async () => ({ methods: [] }), send: async () => ({}),
    async queryTimeline(input) { signals.push(input.signal); return await queries[queryCount++]!; },
    subscribeTimeline(input, deliver) {
      const subscription = { input, deliver, stopped: false };
      subscriptions.push(subscription);
      return () => { subscription.stopped = true; };
    },
  } as MetaConsoleTransport;
  return { transport, subscriptions, signals, queries: () => queryCount };
}
for (const [name, create] of [["shared", sharedController], ["stock", stockController]] as const) {
  test(`${name}: two sequential gaps repair accepted history, coalesce overlap, and reject old live generations`, async () => {
    const repair = deferred<ConsoleTimelinePage>();
    const f = fixture([page(1), repair.promise, page(3)]);
    const controller = create({ transport: f.transport });
    const seen: string[] = [];
    let invalidations = 0;
    const stop = await controller.timeline.subscribeWithBackfill({}, (fact) => seen.push(fact.value.id), () => invalidations++);
    const first = f.subscriptions[0];
    const gap: ConsoleFrame = { id: "", event: "replay_unavailable", data: { reason: "source_reset" } };
    first.deliver(gap);
    first.deliver(gap);
    first.deliver(frame(99));
    assert.equal(first.stopped, true);
    assert.equal(f.queries(), 2);
    repair.resolve({ frames: [frame(1), frame(2)], latestCursor: "console:2", available: true });
    await tick();
    assert.equal(f.subscriptions[1].input.after, "console:2");
    f.subscriptions[1].deliver({ id: "console:2", event: "snapshot_complete", data: {} });
    f.subscriptions[1].deliver(gap);
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(f.queries(), 3);
    assert.equal(f.subscriptions[2].input.after, "console:3");
    assert.deepEqual(seen, ["frame:1", "frame:2", "console:2", "frame:3"]);
    assert.equal(invalidations, 2);
    stop();
  });

  test(`${name}: abort during initial seed returns promptly and discards late authorized results`, async () => {
    const seed = deferred<ConsoleTimelinePage>();
    const f = fixture([seed.promise]);
    const signal = new AbortController();
    const seen: ConsoleFrame[] = [];
    const pending = create({ transport: f.transport }).timeline.subscribeWithBackfill({ signal: signal.signal }, (fact) => seen.push(fact.value));
    signal.abort();
    const stop = await pending;
    assert.equal(f.signals[0]?.aborted, true);
    seed.resolve(page(1));
    await tick();
    assert.deepEqual(seen, []);
    assert.equal(f.subscriptions.length, 0);
    stop();
  });

  test(`${name}: switching scope aborts a repair and cannot inject history into a replacement`, async () => {
    const repair = deferred<ConsoleTimelinePage>();
    const f = fixture([page(1), repair.promise, page(10)]);
    const controller = create({ transport: f.transport });
    const old: string[] = [], next: string[] = [];
    const stop = await controller.timeline.subscribeWithBackfill({ identity: "old" }, (fact) => old.push(fact.value.id));
    f.subscriptions[0].deliver({ id: "", event: "replay_unavailable", data: {} });
    stop();
    const stopNext = await controller.timeline.subscribeWithBackfill({ identity: "new" }, (fact) => next.push(fact.value.id));
    repair.resolve(page(2));
    await tick();
    assert.deepEqual(old, ["frame:1"]);
    assert.deepEqual(next, ["frame:10"]);
    assert.equal(f.subscriptions.length, 2);
    stopNext();
  });

  test(`${name}: consumer failure does not commit rejected seed cursor or start a stream`, async () => {
    const f = fixture([{ frames: [frame(1), frame(2)], latestCursor: "console:2", available: true }]);
    const states: ConsoleTransportState[] = [];
    const stop = await create({ transport: f.transport }).timeline.subscribeWithBackfill({
      onTransportState: (state) => states.push(state),
    }, (fact) => { if (fact.value.id === "frame:2") throw new Error("rejected"); });
    assert.equal(states.at(-1)?.phase, "consumer-failed");
    assert.equal(states.at(-1)?.cursor, "console:1");
    assert.equal(f.subscriptions.length, 0);
    stop();
  });

  test(`${name}: denied seed does not probe or subscribe`, async () => {
    const f = fixture([Promise.reject(Object.assign(new Error("denied"), { httpStatus: 403 }))]);
    const states: ConsoleTransportState[] = [];
    const stop = await create({ transport: f.transport }).timeline.subscribeWithBackfill({ onTransportState: (state) => states.push(state) }, () => {});
    assert.equal(states.at(-1)?.phase, "forbidden");
    assert.equal(f.queries(), 1);
    assert.equal(f.subscriptions.length, 0);
    stop();
  });
}

for (const [name, create] of [["shared", sharedController], ["stock", stockController]] as const) {
  test(`${name}: failed history repair is visible and cancellation clears its backoff`, async () => {
    const f = fixture([page(1)]);
    // Defer rejection until query starts, avoiding a deliberately pre-rejected fixture promise.
    f.transport.queryTimeline = async (input) => {
      if (input.mode === "recent" && f.subscriptions.length) throw new Error("store offline");
      return page(1);
    };
    const states: ConsoleTransportState[] = [];
    const stop = await create({ transport: f.transport }).timeline.subscribeWithBackfill({ onTransportState: (state) => states.push(state) }, () => {});
    f.subscriptions[0].deliver({ id: "", event: "replay_unavailable", data: {} });
    await tick();
    assert.equal(states.at(-1)?.phase, "retrying");
    assert.match(String((states.at(-1)?.error as Error)?.message), /store offline/);
    assert.equal(f.subscriptions[0].stopped, true);
    stop();
    assert.equal(states.at(-1)?.phase, "stopped");
  });
}

test("rapid repeated gaps stop after bounded repair even if every connection has a snapshot marker", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout", "Date"] });
  const f = fixture(Array.from({ length: 7 }, (_, i) => page(i + 1)));
  const states: ConsoleTransportState[] = [];
  const stop = await sharedController({ transport: f.transport }).timeline.subscribeWithBackfill({ onTransportState: (state) => states.push(state) }, () => {});
  const flush = async () => { for (let i = 0; i < 15; i++) await Promise.resolve(); };
  try {
    for (let cycle = 0; cycle < 6; cycle++) {
      const current = f.subscriptions.at(-1)!;
      current.deliver({ id: `snapshot:${cycle}`, event: "snapshot_complete", data: {} });
      current.deliver({ id: "", event: "replay_unavailable", data: {} });
      context.mock.timers.tick(4000);
      await flush();
    }
    assert.equal(f.queries(), 6);
    assert.equal(f.subscriptions.length, 6);
    assert.equal(states.at(-1)?.phase, "stopped");
    assert.match((states.at(-1)?.error as Error).message, /Repeated timeline gaps/);
  } finally { stop(); }
});

test("repair accepts a new revision of a previously delivered logical frame", async () => {
  const original = { ...frame(1), frameVersion: 1, data: "pending" };
  const revised = { ...frame(1), frameVersion: 2, data: "delivered" };
  const f = fixture([
    { frames: [original], latestCursor: "console:1", available: true },
    { frames: [revised], latestCursor: "console:2", available: true },
  ]);
  const seen: unknown[] = [];
  const stop = await sharedController({ transport: f.transport }).timeline.subscribeWithBackfill({}, (fact) => seen.push(fact.value.data));
  f.subscriptions[0].deliver({ id: "", event: "replay_unavailable", data: {} });
  await tick();
  assert.deepEqual(seen, ["pending", "delivered"]);
  assert.equal(f.subscriptions.at(-1)?.input.after, "console:2");
  stop();
});
