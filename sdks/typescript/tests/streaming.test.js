import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { AsyncQueue, EventStream } from "../dist/streaming.js";
import {
  HostUnavailableError,
  MeerkatError,
  ScopeDeniedError,
  StaleCursorError,
  StaleFenceError,
  meerkatErrorFromJsonRpcCode,
  meerkatErrorFromSemanticCode,
} from "../dist/index.js";

function agentEventEnvelope(payload, eventId = "00000000-0000-4000-8000-000000000010") {
  return {
    event_id: eventId,
    source: { type: "callback" },
    seq: 0,
    timestamp_ms: 1,
    payload,
  };
}

const MULTI_HOST_ERROR_CASES = [
  {
    rpcCode: -32025,
    semanticCode: "SCOPE_DENIED",
    ErrorClass: ScopeDeniedError,
    details: { required: "live", presented: ["list", "read_history"] },
    malformed: { required: "live" },
  },
  {
    rpcCode: -32026,
    semanticCode: "HOST_UNAVAILABLE",
    ErrorClass: HostUnavailableError,
    details: { host: "host:remote-1", timeout_ms: 5000 },
    malformed: { timeout_ms: -1 },
  },
  {
    rpcCode: -32027,
    semanticCode: "STALE_CURSOR",
    ErrorClass: StaleCursorError,
    details: { watermark: 42, generation: 3, requested: 7 },
    malformed: { generation: 3 },
  },
  {
    rpcCode: -32028,
    semanticCode: "STALE_FENCE",
    ErrorClass: StaleFenceError,
    details: { runtime_id: "runtime-1", expected: 9, actual: 8 },
    malformed: { runtime_id: 7 },
  },
];

describe("EventStream late drain", () => {
  it("delivers late tail events after the response resolves without leaking waiters", async () => {
    const queue = new AsyncQueue();
    let resolveResponse;
    const responsePromise = new Promise((resolve) => {
      resolveResponse = resolve;
    });

    const stream = new EventStream({
      sessionId: "s-late",
      eventQueue: queue,
      responsePromise,
      parseResult: () => ({
        sessionId: "s-late",
        text: "done",
        turns: 1,
        toolCalls: 0,
        usage: { inputTokens: 1, outputTokens: 1 },
      }),
    });

    const seen = [];
    const consume = (async () => {
      for await (const event of stream) {
        seen.push(event);
      }
      return stream.result;
    })();

    await new Promise((resolve) => setTimeout(resolve, 0));
    resolveResponse({
      session_id: "s-late",
      text: "done",
      turns: 1,
      tool_calls: 0,
      usage: { input_tokens: 1, output_tokens: 1 },
    });
    queue.put(agentEventEnvelope({ type: "text_delta", delta: "tail" }));

    const result = await consume;
    assert.deepEqual(
      seen.map((event) => event.type),
      ["text_delta"],
      "late tail events should still be yielded during the post-response drain window",
    );
    assert.equal(seen[0].delta, "tail");
    assert.equal(result.text, "done");
    assert.equal(queue.waiters.length, 0, "completed streams must not leak queue waiters");
  });
});

describe("RPC error payload parsing", () => {
  it("uses only the typed error.data projection, never message-string JSON", async () => {
    const { MeerkatClient } = await import("../dist/client.js");
    const parse = MeerkatClient["parseRpcErrorPayload"];
    assert.equal(typeof parse, "function");

    // Typed error.data projection wins.
    const typed = parse({
      code: -32600,
      message: "presentation text",
      data: { code: "TYPED_CODE", message: "typed message", details: { k: "v" } },
    });
    assert.equal(typed.code, "TYPED_CODE");
    assert.equal(typed.message, "typed message");
    assert.deepEqual(typed.details, { k: "v" });

    // A JSON-looking error.message must NOT be recovered into typed fields:
    // error.message is presentation text only.
    const embedded = JSON.stringify({
      code: "FOLKLORE",
      message: "from-message",
      details: { x: 1 },
    });
    const folklore = parse({ code: -32600, message: embedded });
    assert.equal(folklore.code, "-32600");
    assert.equal(folklore.message, embedded);
    assert.equal(folklore.details, undefined);
  });

  it("maps all typed multi-host errors by semantic and numeric code", () => {
    for (const testCase of MULTI_HOST_ERROR_CASES) {
      const semantic = meerkatErrorFromSemanticCode(
        testCase.semanticCode,
        "typed message",
        testCase.details,
      );
      assert.ok(semantic instanceof testCase.ErrorClass);
      assert.deepEqual(semantic.details, testCase.details);

      const numeric = meerkatErrorFromJsonRpcCode(
        testCase.rpcCode,
        undefined,
        "numeric message",
        testCase.details,
      );
      assert.ok(numeric instanceof testCase.ErrorClass);
    }
  });

  it("dispatches all four canonical JSON-RPC errors as typed subclasses", async () => {
    const { MeerkatClient } = await import("../dist/client.js");
    const client = new MeerkatClient();
    let requestId = 100;
    for (const testCase of MULTI_HOST_ERROR_CASES) {
      requestId += 1;
      const response = client.registerRequest(requestId);
      client.handleLine(
        JSON.stringify({
          jsonrpc: "2.0",
          id: requestId,
          error: {
            code: testCase.rpcCode,
            message: "presentation text",
            data: {
              code: testCase.semanticCode,
              message: "typed message",
              details: testCase.details,
            },
          },
        }),
      );
      await assert.rejects(response, (error) => {
        assert.ok(error instanceof testCase.ErrorClass);
        assert.equal(error.code, testCase.semanticCode);
        assert.equal(error.message, "typed message");
        assert.deepEqual(error.details, testCase.details);
        return true;
      });
    }
  });

  it("keeps malformed details generic for every typed multi-host code", async () => {
    const { MeerkatClient } = await import("../dist/client.js");
    const client = new MeerkatClient();
    let requestId = 200;
    for (const testCase of MULTI_HOST_ERROR_CASES) {
      requestId += 1;
      const response = client.registerRequest(requestId);
      client.handleLine(
        JSON.stringify({
          jsonrpc: "2.0",
          id: requestId,
          error: {
            code: testCase.rpcCode,
            message: "presentation text",
            data: {
              code: testCase.semanticCode,
              message: "typed message",
              details: testCase.malformed,
            },
          },
        }),
      );
      await assert.rejects(response, (error) => {
        assert.equal(error.constructor, MeerkatError);
        assert.equal(error.code, testCase.semanticCode);
        assert.deepEqual(error.details, testCase.malformed);
        return true;
      });
    }

    const mismatched = meerkatErrorFromJsonRpcCode(
      -32025,
      "HOST_UNAVAILABLE",
      "mismatched envelope",
      { host: "host:remote-1" },
    );
    assert.equal(mismatched.constructor, MeerkatError);
  });

  it("#1105: MEMBER_RELOAD_REQUIRED reuses generic MeerkatError, preserves full details, distinct from SESSION_BUSY", () => {
    const details = {
      kind: "mob_member_reload_required",
      member_id: "m1",
      reason: "durable commit could not be reconciled with the live shell",
      retryable: false,
      authority_retained: true,
      required_action: "reload_member_registration",
    };

    const semantic = meerkatErrorFromSemanticCode("MEMBER_RELOAD_REQUIRED", "typed message", details);
    assert.equal(semantic.constructor, MeerkatError);
    assert.equal(semantic.code, "MEMBER_RELOAD_REQUIRED");
    assert.deepEqual(semantic.details, details);
    assert.equal(semantic.details.retryable, false);
    assert.equal(semantic.details.required_action, "reload_member_registration");

    const viaNumeric = meerkatErrorFromJsonRpcCode(-32029, "MEMBER_RELOAD_REQUIRED", "typed message", details);
    assert.equal(viaNumeric.constructor, MeerkatError);
    assert.equal(viaNumeric.code, "MEMBER_RELOAD_REQUIRED");
    assert.deepEqual(viaNumeric.details, details);

    const sessionBusy = meerkatErrorFromSemanticCode("SESSION_BUSY", "turn already in progress", {
      stage: "turn_active",
    });
    assert.equal(sessionBusy.constructor, MeerkatError);
    assert.equal(sessionBusy.code, "SESSION_BUSY");
    assert.notEqual(sessionBusy.code, semantic.code);
  });

  it("#1105: dispatches MEMBER_RELOAD_REQUIRED's bare structured_data() wire shape with details preserved, not narrowed to `reason`", async () => {
    const { MeerkatClient } = await import("../dist/client.js");
    const client = new MeerkatClient();
    const requestId = 301;
    const details = {
      kind: "mob_member_reload_required",
      member_id: "m1",
      reason: "durable commit could not be reconciled with the live shell",
      retryable: false,
      authority_retained: true,
      required_action: "reload_member_registration",
    };
    const response = client.registerRequest(requestId);
    // The real wire shape (`meerkat-mob/src/error.rs` `mob_call_error`, no
    // `wire_detail()` match): `error.data` is the BARE structured_data()
    // dict directly — no {code, message, details} envelope.
    client.handleLine(
      JSON.stringify({
        jsonrpc: "2.0",
        id: requestId,
        error: {
          code: -32029,
          message:
            "member m1 requires a runtime reload before it can accept work: durable commit could not be reconciled with the live shell",
          data: details,
        },
      }),
    );
    await assert.rejects(response, (error) => {
      assert.equal(error.constructor, MeerkatError);
      assert.equal(error.code, "-32029");
      assert.deepEqual(error.details, details);
      assert.equal(error.details.retryable, false);
      assert.equal(error.details.required_action, "reload_member_registration");
      assert.equal(error.details.kind, "mob_member_reload_required");
      return true;
    });
  });

  it("#1105: SESSION_BUSY mob dispatch (RetirementInProgress, no `reason` key) is unchanged by the details-preservation fix", async () => {
    const { MeerkatClient } = await import("../dist/client.js");
    const client = new MeerkatClient();
    const requestId = 302;
    const details = {
      kind: "mob_retirement_in_progress",
      session_id: "s1",
      stage: "session_archive_and_unregister",
      deadline_reached: true,
      retryable: true,
      authority_retained: true,
    };
    const response = client.registerRequest(requestId);
    client.handleLine(
      JSON.stringify({
        jsonrpc: "2.0",
        id: requestId,
        error: {
          code: -32002,
          message: "retirement in progress",
          data: details,
        },
      }),
    );
    await assert.rejects(response, (error) => {
      assert.equal(error.constructor, MeerkatError);
      assert.equal(error.code, "-32002");
      assert.deepEqual(error.details, details);
      return true;
    });
  });
});

describe("EventSubscription close authority", () => {
  it("awaits server stream-close authority before surfacing closed state", async () => {
    const { EventSubscription } = await import("../dist/subscription.js");

    // Rejected close: typed error propagates, subscription stays open.
    let queue = new AsyncQueue();
    const rejecting = new EventSubscription({
      streamId: "stream-1",
      queue,
      closeRemote: async () => {
        throw new Error("server said no");
      },
      parseEvent: (raw) => raw,
      getTerminalOutcome: () => undefined,
    });
    await assert.rejects(() => rejecting.close(), /server said no/);
    assert.equal(rejecting.isClosed, false, "rejected close must not surface closed state");
    assert.equal(queue.buffer.length, 0, "rejected close must not end the local queue");

    // Accepted close flips closed state and ends the queue; close is
    // retryable after a rejection (single in-flight close at a time).
    queue = new AsyncQueue();
    const order = [];
    const accepting = new EventSubscription({
      streamId: "stream-2",
      queue,
      closeRemote: async () => {
        order.push("remote");
      },
      parseEvent: (raw) => raw,
      getTerminalOutcome: () => undefined,
    });
    await accepting.close();
    order.push("local-closed:" + accepting.isClosed);
    assert.deepEqual(order, ["remote", "local-closed:true"]);
    assert.equal(await queue.get(), null, "accepted close ends the local queue");
  });
});
