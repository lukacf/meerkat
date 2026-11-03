import assert from "node:assert/strict";
import test from "node:test";
import { ConsoleConsumerError, consumeSseResponse } from "./sse-reader";
import { parseSseFrames } from "./network";

function responseChunks(chunks: Uint8Array[], onCancel = () => {}) {
  return new Response(new ReadableStream<Uint8Array>({
    start(controller) { for (const chunk of chunks) controller.enqueue(chunk); controller.close(); },
    cancel: onCancel,
  }));
}
const encoder = new TextEncoder();

test("SSE consumption delivers 25,000 frames without returning a collector", async () => {
  let count = 0;
  const chunks = Array.from({ length: 250 }, (_, chunk) => encoder.encode(
    Array.from({ length: 100 }, (_, offset) => `id: ${chunk * 100 + offset}\nevent: text_delta\ndata: {}\n\n`).join(""),
  ));
  const result = await consumeSseResponse(responseChunks(chunks), {
    mode: "consume", parseBlock: parseSseFrames,
    onFrame: (frame) => { assert.equal(frame.id, String(count++)); },
  });
  assert.equal(count, 25_000);
  assert.equal(result, undefined);
});

test("finite collection stops on its matching terminal in the middle of a chunk", async () => {
  const response = responseChunks([encoder.encode(
    'id: other\nevent: run_completed\ndata: {"session_id":"other"}\n\n' +
    'id: matching\nevent: run_completed\ndata: {"session_id":"wanted"}\n\n' +
    'id: after\nevent: text_delta\ndata: {}\n\n',
  )]);
  const result = await consumeSseResponse(response, {
    mode: "collect", parseBlock: parseSseFrames,
    accept: (frame) => (frame.data as { session_id?: string }).session_id === "wanted",
    terminal: (frame) => frame.event === "run_completed",
  });
  assert.deepEqual(result.map((frame) => frame.id), ["matching"]);
});

test("SSE preserves split UTF-8, CRLF, trailing events and decoder flush", async () => {
  const bytes = encoder.encode('id: one\r\nevent: text_delta\r\ndata: {"text":"🐾 café"}\r\n\r\nid: two\ndata: final');
  // Every byte is a chunk, exercising both delimiter and multibyte splits.
  const result = await consumeSseResponse(responseChunks(Array.from(bytes, (byte) => new Uint8Array([byte]))), {
    mode: "collect", parseBlock: parseSseFrames,
  });
  assert.deepEqual(result.map((frame) => [frame.id, frame.data]), [["one", { text: "🐾 café" }], ["two", "final"]]);
});

test("consumer failure cancels a still-open reader and preserves the error category", async () => {
  let cancelled = false;
  const response = new Response(new ReadableStream({
    start(controller) { controller.enqueue(encoder.encode("id: one\ndata: {}\n\n")); },
    cancel() { cancelled = true; },
  }));
  await assert.rejects(consumeSseResponse(response, {
    mode: "consume", parseBlock: parseSseFrames,
    onFrame() { throw new Error("consumer rejected"); },
  }), (error) => error instanceof ConsoleConsumerError && error.message === "consumer rejected");
  assert.equal(cancelled, true);
});

test("abort cancels a pending read and never flushes an incomplete frame", async () => {
  let cancelled = false;
  const signal = new AbortController();
  const response = new Response(new ReadableStream({
    start(controller) { controller.enqueue(encoder.encode("id: one\ndata: incomplete")); },
    cancel() { cancelled = true; },
  }));
  let delivered = 0;
  const read = consumeSseResponse(response, {
    mode: "consume", parseBlock: parseSseFrames, signal: signal.signal,
    onFrame() { delivered++; },
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  signal.abort();
  await read;
  assert.equal(cancelled, true);
  assert.equal(delivered, 0);
});

test("promise-returning consumers fail explicitly without unhandled rejection", async () => {
  await assert.rejects(consumeSseResponse(responseChunks([encoder.encode("id: one\ndata: {}\n\n")]), {
    mode: "consume", parseBlock: parseSseFrames,
    onFrame: (async () => { throw new Error("async acceptance is unsupported"); }) as () => void,
  }), (error) => error instanceof ConsoleConsumerError && /synchronously/.test(error.message));
});
