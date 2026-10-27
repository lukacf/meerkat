import assert from "node:assert/strict";
import test from "node:test";
import * as shared from "./network";
import * as stock from "../../../console/src/lib/network";
import type { ConsoleFrame } from "./runtime-types";

const assistantId = "Opaque/Assistant-ID ";
const payload = { assistant_message_id: assistantId, text: "  A\u030A\n🚀  ", id: "provider-item", image: { image_id: "image-block" } };
const wireFrame = (kind = "text_delta", data: unknown = payload, session = "session-a") => ({
  id: `frame-${kind}`, cursor: `console:${kind}`, kind, session_id: session,
  identity: "router:main", runtime_key: "runtime-a", interaction_id: "interaction", run_id: "run",
  source: { kind: "console_event" }, payload: data,
});
const sse = (frame: unknown, event = "console_frame") => `event: ${event}\ndata: ${JSON.stringify({ type: "console_frame", frame })}\n\n`;
const nested = (input: ConsoleFrame): ConsoleFrame => (input.data as { frame: ConsoleFrame }).frame;

for (const [name, transport] of [["shared", shared], ["stock", stock]] as const) {
  test(`${name}: assistant carriers stay byte-exact in every live event payload`, () => {
    for (const kind of ["run_completed", "interaction_complete", "turn_started", "reasoning_delta", "reasoning_complete", "text_delta", "text_complete", "server_tool_content", "assistant_image_appended", "turn_completed", "retrying"]) {
      const [frame] = transport.parseSseFrames(sse(wireFrame(kind)));
      assert.deepEqual(frame.data, payload, kind);
      assert.equal(frame.sessionId, "session-a");
      assert.equal(frame.id, `frame-${kind}`);
      assert.equal("assistantMessageId" in frame, false, "no redundant lifted identity");
    }
  });

  test(`${name}: history and replay update normalization preserve containing and block identities`, async () => {
    const originalFetch = globalThis.fetch;
    const canonical = wireFrame("text_complete", {
      ...payload,
      message: { role: "block_assistant", assistant_message_id: assistantId, blocks: [
        { block_type: "text", data: { text: "before\n" } },
        { block_type: "server_tool_content", data: { id: "provider-item", content: "tool output" } },
        { block_type: "image", data: { image_id: "image-block" } },
        { block_type: "text", data: { text: " after" } },
      ] },
    });
    canonical.source.kind = "session_history";
    const update = { ...wireFrame("frame_updated", { frame: canonical }), session_id: "outer-session", id: "update-frame" };
    globalThis.fetch = (async () => Response.json({ jsonrpc: "2.0", id: "query", result: { frames: [canonical, update], available: true } })) as typeof fetch;
    try {
      const page = await transport.queryTimeline("", { identity: "router:main" });
      assert.deepEqual(page.frames[0].data, canonical.payload);
      assert.deepEqual(nested(page.frames[1]).data, canonical.payload);
      assert.equal(nested(page.frames[1]).sessionId, "session-a", "outer update cannot replace the actual frame session");
      for (const event of ["console_frame", "frame_updated"]) {
        const [liveUpdate] = transport.parseSseFrames(sse(update, event));
        assert.deepEqual(nested(liveUpdate).data, canonical.payload);
        assert.equal(nested(liveUpdate).sessionId, "session-a");
      }
    } finally { globalThis.fetch = originalFetch; }
  });

  test(`${name}: malformed and absent carriers remain payload data without coercion or lineage inheritance`, () => {
    for (const data of [{}, { assistant_message_id: null }, { assistant_message_id: "" }, { assistant_message_id: 23 }, { assistant_message_id: { id: "wrong" } }, { image: { assistant_message_id: "image-only" } }]) {
      const [frame] = transport.parseSseFrames(sse(wireFrame("text_delta", data)));
      assert.deepEqual(frame.data, data);
      assert.equal("assistantMessageId" in frame, false);
    }
    const [missingSession] = transport.parseSseFrames(sse({ ...wireFrame(), session_id: undefined }));
    assert.equal(missingSession.sessionId, undefined);
    assert.deepEqual(missingSession.data, payload);
  });

  test(`${name}: streamed chunk boundaries preserve assistant identity and exact Unicode payload`, async () => {
    const originalFetch = globalThis.fetch;
    const bytes = new TextEncoder().encode(sse(wireFrame("text_delta")));
    let cancelled = false;
    globalThis.fetch = (async () => new Response(new ReadableStream<Uint8Array>({
      start(controller) { for (let index = 0; index < bytes.length; index++) controller.enqueue(bytes.slice(index, index + 1)); },
      cancel() { cancelled = true; },
    }))) as typeof fetch;
    let stop = () => {};
    try {
      const received = await new Promise<ConsoleFrame>((resolve, reject) => {
        stop = transport.subscribeTimelineEvents("", { identity: "router:main" }, resolve, {
          onTransportState(state) { if (state.error) reject(state.error); },
        });
      });
      assert.deepEqual(received.data, payload);
      assert.equal(received.sessionId, "session-a");
      stop();
      await new Promise(resolve => setTimeout(resolve, 0));
      assert.equal(cancelled, true);
    } finally { stop(); globalThis.fetch = originalFetch; }
  });
}
