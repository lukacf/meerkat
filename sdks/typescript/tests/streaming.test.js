import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { AsyncQueue, EventStream } from "../dist/streaming.js";

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
    queue.put({ type: "text_delta", delta: "tail" });

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
