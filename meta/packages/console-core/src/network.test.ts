import assert from "node:assert/strict";
import test from "node:test";
import { callConsoleRpc as sharedRpc, subscribeTimelineEvents as sharedSubscribe, sendConsole as sharedSend, sendConsoleMultipart as sharedMultipart } from "./network";
import { callConsoleRpc as stockRpc, subscribeTimelineEvents as stockSubscribe, sendConsole as stockSend, sendConsoleMultipart as stockMultipart } from "../../../console/src/lib/network";
import type { ConsoleTransportState } from "./timeline-subscription";
import type { ConsoleFrame } from "./runtime-types";

const encoder = new TextEncoder();
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
for (const [name, send, multipart] of [
  ["shared", sharedSend, sharedMultipart], ["stock", stockSend, stockMultipart],
] as const) {
  test(`${name}: JSON and multipart sends require an explicit owner receipt and preserve canonical alias resolution`, async () => {
    const original = globalThis.fetch;
    let receipt: unknown;
    const sent: Array<Record<string, unknown>> = [];
    globalThis.fetch = (async (_url, init) => {
      const payload = init?.body instanceof FormData
        ? JSON.parse(String(init.body.get("payload")))
        : JSON.parse(String(init?.body));
      sent.push(payload.params);
      return Response.json({ jsonrpc: "2.0", id: payload.id, result: receipt });
    }) as typeof fetch;
    const invoke = [
      () => send("", "reviewer", "Review this", "console:test", "stable-key"),
      () => multipart("", "reviewer", "Review this", [], "console:test", "stable-key"),
    ];
    try {
      for (const call of invoke) {
        for (receipt of [
          null, {}, { interaction_id: 123 }, { interaction_id: "real-id" },
          { interaction_id: 123, identity: "reviewer" },
          { interaction_id: "  ", identity: "reviewer" },
          { interaction_id: "real-id", identity: " " },
          { interaction_id: "real-id", identity: "reviewer", input_frame_id: 123 },
          { interaction_id: "real-id", identity: "reviewer", input_frame_id: " " },
        ]) await assert.rejects(call, /invalid acceptance payload/);
        receipt = { interaction_id: "owner-interaction", identity: "reviewer", input_frame_id: "owner-frame" };
        const accepted = await call();
        assert.equal(accepted.interaction_id, "owner-interaction");
        assert.equal(accepted.identity, "reviewer");
        assert.equal(accepted.input_frame_id, "owner-frame");
        receipt = { interaction_id: "canonical-interaction", identity: "mob/canonical-reviewer", input_frame_id: "canonical-frame" };
        const canonical = await call();
        assert.equal(canonical.identity, "mob/canonical-reviewer");
        assert.equal(canonical.input_frame_id, "canonical-frame");
      }
      assert.ok(sent.every(params => params.origin_kind === "operator"));
      assert.ok(sent.every(params => params.idempotency_key === "stable-key"));
    } finally { globalThis.fetch = original; }
  });
}
function block(id: number, event = "text_delta") {
  return `id: console:${id}\nevent: console_frame\ndata: ${JSON.stringify({ type: "console_frame", frame: { id: `frame:${id}`, cursor: `console:${id}`, kind: event, payload: { text: String(id) } } })}\n\n`;
}
for (const [name, subscribe] of [["shared", sharedSubscribe], ["stock", stockSubscribe]] as const) {
  test(`${name}: 25,000-frame continuous delivery preserves order through the public transport`, async () => {
    const original = globalThis.fetch;
    let cancelled = false;
    globalThis.fetch = (async () => new Response(new ReadableStream({
      start(controller) {
        for (let start = 0; start < 25_000; start += 100) {
          controller.enqueue(encoder.encode(Array.from({ length: 100 }, (_, offset) => block(start + offset)).join("")));
        }
      }, cancel() { cancelled = true; },
    }))) as typeof fetch;
    let count = 0;
    const states: ConsoleTransportState[] = [];
    let stop = () => {};
    try {
      const complete = new Promise<void>((resolve) => {
        stop = subscribe("", {}, (frame) => {
          assert.equal(frame.cursor, `console:${count++}`);
          if (count === 25_000) resolve();
        }, { onTransportState: (state) => states.push(state) });
      });
      await complete;
      stop();
      await tick();
      assert.equal(count, 25_000);
      assert.equal(cancelled, true);
      assert.equal(states.some((state) => state.phase === "live"), false, "frames alone cannot prove replay completion");
    } finally { stop(); globalThis.fetch = original; }
  });

  for (const httpStatus of [401, 403, 503]) {
    test(`${name}: HTTP ${httpStatus} reports transport state and never invents replay loss`, async () => {
      const original = globalThis.fetch;
      const frames: ConsoleFrame[] = [];
      const states: ConsoleTransportState[] = [];
      let requests = 0;
      globalThis.fetch = (async () => { requests++; return new Response("unavailable", { status: httpStatus }); }) as typeof fetch;
      const stop = subscribe("", {}, (frame) => frames.push(frame), { onTransportState: (state) => states.push(state) });
      try {
        await tick();
        assert.equal(states.at(-1)?.phase, httpStatus === 401 ? "authentication-required" : httpStatus === 403 ? "forbidden" : "retrying");
        assert.equal(states.at(-1)?.httpStatus, httpStatus);
        assert.deepEqual(frames, []);
        assert.equal(requests, 1);
      } finally { stop(); globalThis.fetch = original; }
    });
  }

  test(`${name}: accepted snapshot marker is required for live and callback throw retains accepted cursor`, async () => {
    const original = globalThis.fetch;
    let requests = 0;
    let cancelled = false;
    globalThis.fetch = (async () => {
      requests++;
      return new Response(new ReadableStream({
        start(controller) { controller.enqueue(encoder.encode(block(1) + 'id: console:2\nevent: snapshot_complete\ndata: {"cursor":"console:2"}\n\n' + block(3))); },
        cancel() { cancelled = true; },
      }));
    }) as typeof fetch;
    const states: ConsoleTransportState[] = [];
    const stop = subscribe("", {}, (frame) => {
      if (frame.event === "snapshot_complete") throw new Error("snapshot rejected");
    }, { onTransportState: (state) => states.push(state) });
    try {
      await tick();
      assert.equal(stop.cursor(), "console:1");
      assert.equal(states.at(-1)?.phase, "consumer-failed");
      assert.equal(states.some((state) => state.phase === "live"), false);
      assert.equal(cancelled, true);
      assert.equal(requests, 1);
    } finally { stop(); globalThis.fetch = original; }
  });

  test(`${name}: typed replay errors stop at the accepted cursor until the controller repairs history`, async () => {
    const original = globalThis.fetch;
    const frames: ConsoleFrame[] = [];
    let requests = 0;
    globalThis.fetch = (async () => {
      requests++;
      return new Response(JSON.stringify({ error: "replay_unavailable", requested_cursor: "console:1", latest_cursor: "console:900" }), { status: 409 });
    }) as typeof fetch;
    const stop = subscribe("", { after: "console:1" }, (frame) => frames.push(frame));
    try {
      await tick();
      assert.equal(frames[0]?.event, "replay_unavailable");
      assert.equal(stop.cursor(), "console:1");
      assert.equal(requests, 1);
    } finally { stop(); globalThis.fetch = original; }
  });

  test(`${name}: cursorless source resets remain protocol gaps`, async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () => new Response('event: replay_unavailable\ndata: {"reason":"source_reset","source_kind":"agent"}\n\n')) as typeof fetch;
    const frames: ConsoleFrame[] = [];
    const stop = subscribe("", {}, (frame) => frames.push(frame));
    try {
      await tick();
      assert.equal(frames.length, 1);
      assert.deepEqual(frames[0].data, { reason: "source_reset", source_kind: "agent" });
    } finally { stop(); globalThis.fetch = original; }
  });

  test(`${name}: snapshot completion establishes live, terminal turn does not end a subscription`, async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () => new Response(new ReadableStream({
      start(controller) { controller.enqueue(encoder.encode('id: console:1\nevent: snapshot_complete\ndata: {"cursor":"console:1"}\n\n' + block(2, "run_completed") + block(3))); },
    }))) as typeof fetch;
    const states: ConsoleTransportState[] = [];
    const frames: ConsoleFrame[] = [];
    const stop = subscribe("", {}, (frame) => frames.push(frame), { onTransportState: (state) => states.push(state) });
    try {
      await tick();
      assert.equal(states.at(-1)?.phase, "live");
      assert.deepEqual(frames.map((frame) => frame.event), ["snapshot_complete", "run_completed", "text_delta"]);
    } finally { stop(); globalThis.fetch = original; }
  });
}

for (const [name, subscribe] of [["shared", sharedSubscribe], ["stock", stockSubscribe]] as const) {
  for (const rejected of [1, 2]) {
    test(`${name}: throw on frame ${rejected} stops retry and keeps the last accepted frontier`, async () => {
      const original = globalThis.fetch;
      const states: ConsoleTransportState[] = [];
      globalThis.fetch = (async () => new Response(block(1) + block(2) + block(3))) as typeof fetch;
      const seen: string[] = [];
      const stop = subscribe("", { after: "console:0" }, (frame) => {
        if (frame.cursor === `console:${rejected}`) throw new Error("rejected");
        seen.push(frame.cursor!);
      }, { onTransportState: (state) => states.push(state) });
      try {
        await tick();
        assert.equal(states.at(-1)?.phase, "consumer-failed");
        assert.equal(stop.cursor(), `console:${rejected - 1}`);
        assert.deepEqual(seen, rejected === 1 ? [] : ["console:1"]);
      } finally { stop(); globalThis.fetch = original; }
    });
  }

  test(`${name}: explicit retry wakes transient backoff and resumes from accepted frames`, async () => {
    const original = globalThis.fetch;
    const headers: Array<string | undefined> = [];
    let calls = 0;
    globalThis.fetch = (async (_url, init) => {
      headers.push((init?.headers as Record<string, string>)["Last-Event-ID"]);
      if (++calls === 1) return new Response(block(1));
      return new Response(new ReadableStream({ start(controller) { controller.enqueue(encoder.encode(block(2))); } }));
    }) as typeof fetch;
    const states: ConsoleTransportState[] = [];
    const seen: string[] = [];
    const stop = subscribe("", {}, (frame) => seen.push(frame.cursor!), { onTransportState: (state) => states.push(state) });
    try {
      await tick();
      assert.equal(states.at(-1)?.phase, "retrying");
      stop.retry();
      await tick();
      assert.deepEqual(headers, [undefined, "console:1"]);
      assert.deepEqual(seen, ["console:1", "console:2"]);
    } finally { stop(); globalThis.fetch = original; }
  });

  test(`${name}: offline status suspends fetches and disposal removes the online listener`, async () => {
    const originalFetch = globalThis.fetch;
    const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");
    const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
    const events = new EventTarget();
    let online = false;
    Object.defineProperty(globalThis, "navigator", { configurable: true, value: { get onLine() { return online; } } });
    Object.defineProperty(globalThis, "window", { configurable: true, value: events });
    let calls = 0;
    globalThis.fetch = (async () => { calls++; return new Response(new ReadableStream()); }) as typeof fetch;
    const states: ConsoleTransportState[] = [];
    const stop = subscribe("", {}, () => {}, { onTransportState: (state) => states.push(state) });
    try {
      await tick();
      assert.equal(calls, 0);
      assert.equal(states.at(-1)?.phase, "offline");
      online = true;
      events.dispatchEvent(new Event("online"));
      await tick();
      assert.equal(calls, 1);
      stop();
      events.dispatchEvent(new Event("online"));
      await tick();
      assert.equal(calls, 1);
    } finally {
      stop(); globalThis.fetch = originalFetch;
      if (originalNavigator) Object.defineProperty(globalThis, "navigator", originalNavigator); else Reflect.deleteProperty(globalThis, "navigator");
      if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow); else Reflect.deleteProperty(globalThis, "window");
    }
  });

  test(`${name}: caller abort cancels a pending fetch before any frames`, async () => {
    const original = globalThis.fetch;
    let requestSignal: AbortSignal | undefined;
    globalThis.fetch = (async (_url, init) => {
      requestSignal = init?.signal || undefined;
      return await new Promise<Response>((_resolve, reject) => requestSignal!.addEventListener("abort", () => reject(new Error("aborted"))));
    }) as typeof fetch;
    const caller = new AbortController();
    const states: ConsoleTransportState[] = [];
    const stop = subscribe("", {}, () => assert.fail("unexpected frame"), { signal: caller.signal, onTransportState: (state) => states.push(state) });
    try {
      caller.abort();
      await tick();
      assert.equal(requestSignal?.aborted, true);
      assert.equal(states.at(-1)?.phase, "stopped");
    } finally { stop(); globalThis.fetch = original; }
  });
}

for (const [name, subscribe] of [["shared", sharedSubscribe], ["stock", stockSubscribe]] as const) {
  test(`${name}: typed cursor loss with no latest frontier still reaches the repair owner`, async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () => new Response(JSON.stringify({ error: "replay_unavailable", requested_cursor: "console:4", latest_cursor: null }), { status: 409 })) as typeof fetch;
    const frames: ConsoleFrame[] = [];
    const stop = subscribe("", { after: "console:4" }, (frame) => frames.push(frame));
    try {
      await tick();
      assert.equal(frames[0]?.event, "replay_unavailable");
      assert.equal(stop.cursor(), "console:4");
    } finally { stop(); globalThis.fetch = original; }
  });
}

for (const [name, rpc] of [["shared", sharedRpc], ["stock", stockRpc]] as const) {
  test(`${name}: typed JSON-RPC errors keep the owner's structured data for checked access saves`, async () => {
    const original = globalThis.fetch;
    const errors = [
      { code: -32004, message: "Access changes are temporarily unavailable.", data: { kind: "access_mutation_unavailable" } },
      { code: -32602, message: "Invalid access configuration.", data: { kind: "invalid_access_config" } },
      { code: -32602, message: "invalid access config: missing field" },
    ];
    try {
      for (const error of errors) {
        globalThis.fetch = (async (_url, init) => Response.json({
          jsonrpc: "2.0", id: JSON.parse(String(init?.body)).id, error,
        })) as typeof fetch;
        await assert.rejects(rpc("", "meta/access/set", { checked_v1: {} }), (thrown: unknown) => {
          assert.deepEqual((thrown as { rpcError?: unknown }).rpcError, error);
          return true;
        });
      }
    } finally { globalThis.fetch = original; }
  });
}
