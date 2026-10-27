import { describe, it } from "node:test";
import assert from "node:assert/strict";

import {
  RpcError,
  MobEventsStaleError,
  ConsoleTimelineReplayUnavailableError,
  WorkGraphUnavailableError,
  isRpcError,
  isMobEventsStaleError,
  MOB_EVENTS_STALE_CURSOR_CODE,
} from "../dist/index.js";
import { parseSseStream } from "../dist/sse.js";
import { parseSubscribeResult, parseDispatchInput } from "../dist/types.js";

// -- Bug #5: SSE parser drops `id:`/`event:` without space --------------

describe("parseSseStream (bug-hunt regression)", () => {
  it("parses `id:N\\nevent:foo\\ndata:bar\\n\\n` (no space after colon)", async () => {
    async function* source(): AsyncGenerator<Uint8Array> {
      yield new TextEncoder().encode(
        "id:42\nevent:text_delta\ndata:{\"type\":\"text_delta\",\"delta\":\"x\"}\n\n",
      );
    }
    const events: Array<{ id: string | null; event: string; data: string }> = [];
    for await (const e of parseSseStream(source())) {
      events.push({ id: e.id, event: e.event, data: e.data });
    }
    assert.equal(events.length, 1);
    assert.equal(
      events[0].id,
      "42",
      "id: without space must be parsed; pre-fix the line was silently dropped",
    );
    assert.equal(events[0].event, "text_delta");
    assert.equal(events[0].data, '{"type":"text_delta","delta":"x"}');
  });

  it("still parses `id: N` with the optional space", async () => {
    async function* source(): AsyncGenerator<Uint8Array> {
      yield new TextEncoder().encode("id: 99\nevent: ping\ndata: ok\n\n");
    }
    const events = [];
    for await (const e of parseSseStream(source())) {
      events.push(e);
    }
    assert.equal(events[0].id, "99");
    assert.equal(events[0].event, "ping");
    assert.equal(events[0].data, "ok");
  });
});

// -- Bug #6: parseSubscribeResult silent empty envelopes ----------------

describe("parseSubscribeResult (bug-hunt regression)", () => {
  it("drops non-object entries instead of producing empty envelopes", () => {
    const result = parseSubscribeResult({
      scope: "mob",
      replay_from_event_id: null,
      keep_alive: { interval_ms: 1000, event: "k" },
      keep_alive_comment: "",
      event_frames: [],
      events: [
        null,
        "oops",
        42,
        { event_id: "e1", source: "a", timestamp_ms: 1, event: {} },
      ],
    });
    assert.equal(
      result.events.length,
      1,
      "non-object entries (null, string, number) must be dropped, not coerced into empty envelopes",
    );
    assert.equal(result.events[0].eventId, "e1");
  });
});

// -- Bug #20: instanceof RpcError cross-module hazard -------------------

describe("isRpcError / isMobEventsStaleError (bug-hunt regression)", () => {
  it("recognizes a fresh-module RpcError instance via structural check", () => {
    // Simulate a cross-module-copy: a different RpcError class with
    // the same shape. `instanceof` would fail; `isRpcError` succeeds.
    class ForeignRpcError extends Error {
      readonly name = "RpcError";
      constructor(
        readonly code: number,
        message: string,
        readonly requestId: string,
        readonly method: string,
        readonly data?: unknown,
      ) {
        super(message);
      }
    }
    const foreign = new ForeignRpcError(-32001, "x", "rid", "m");
    assert.ok(
      !(foreign instanceof RpcError),
      "precondition: cross-module instance is NOT instanceof local RpcError",
    );
    assert.ok(
      isRpcError(foreign),
      "isRpcError must accept a structurally compatible RpcError",
    );
  });

  it("recognizes a fresh-module MobEventsStaleError", () => {
    class ForeignStale extends Error {
      readonly name = "MobEventsStaleError";
      readonly code = MOB_EVENTS_STALE_CURSOR_CODE;
      constructor(public message: string) {
        super(message);
      }
    }
    const foreign = new ForeignStale("stale");
    assert.ok(!(foreign instanceof MobEventsStaleError));
    assert.ok(isMobEventsStaleError(foreign));
  });

  it("rejects a plain Error", () => {
    assert.equal(isRpcError(new Error("nope")), false);
  });

  it("recognizes a console timeline replay-unavailable error", () => {
    const err = new ConsoleTimelineReplayUnavailableError(
      "replay unavailable",
      "rid",
      "meta/console/query_timeline",
    );
    assert.ok(isRpcError(err));
    assert.equal(err.code, -32013);
  });

  it("recognizes a workgraph-unavailable error", () => {
    const err = new WorkGraphUnavailableError(
      "workgraph service not configured",
      "rid",
      "meta/workgraph/snapshot",
    );
    assert.ok(isRpcError(err));
    assert.equal(err.code, -32041);
  });
});

// -- Bug #25: parseDispatchInput unchecked origin cast ------------------

describe("parseDispatchInput (bug-hunt regression)", () => {
  it("rejects unknown origin and falls back to system", () => {
    const result = parseDispatchInput({ origin: "garbage", content: "hi" });
    assert.equal(
      result.origin,
      "system",
      "unknown origin must fall back to system; pre-fix it was cast through silently",
    );
  });

  it("preserves valid origin values", () => {
    for (const origin of ["connector", "scheduler", "policy", "flow", "system"] as const) {
      const result = parseDispatchInput({ origin, content: "" });
      assert.equal(result.origin, origin);
    }
  });
});
