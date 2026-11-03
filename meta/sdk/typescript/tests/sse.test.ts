import { describe, it } from "node:test";
import assert from "node:assert/strict";

import { parseSseStream, encodeSseEvent, TransportError } from "../dist/index.js";
import type { SseEvent } from "../dist/index.js";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Convert a string into an async iterable of Uint8Array chunks. */
function toChunks(...parts: string[]): AsyncIterable<Uint8Array> {
  const encoder = new TextEncoder();
  return {
    async *[Symbol.asyncIterator]() {
      for (const part of parts) {
        yield encoder.encode(part);
      }
    },
  };
}

/** An async iterable over the given byte chunks. */
function byteChunks(...parts: Uint8Array[]): AsyncIterable<Uint8Array> {
  return {
    async *[Symbol.asyncIterator]() {
      for (const part of parts) {
        yield part;
      }
    },
  };
}

/** Collect all events from the async generator. */
async function collectEvents(
  source: AsyncIterable<Uint8Array>,
): Promise<SseEvent[]> {
  const events: SseEvent[] = [];
  for await (const event of parseSseStream(source)) {
    events.push(event);
  }
  return events;
}

// ---------------------------------------------------------------------------
// parseSseStream
// ---------------------------------------------------------------------------

describe("parseSseStream", () => {
  it("parses a basic event", async () => {
    const raw = "data: hello\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].data, "hello");
    assert.equal(events[0].event, "message");
    assert.equal(events[0].id, null);
  });

  it("parses multi-line data", async () => {
    const raw = "data: line1\ndata: line2\ndata: line3\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].data, "line1\nline2\nline3");
  });

  it("parses custom event name", async () => {
    const raw = "event: custom\ndata: payload\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].event, "custom");
    assert.equal(events[0].data, "payload");
  });

  it("parses event with id", async () => {
    const raw = "id: 42\ndata: identified\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].id, "42");
    assert.equal(events[0].data, "identified");
  });

  it("skips comment lines", async () => {
    const raw = ": this is a comment\ndata: real data\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].data, "real data");
  });

  it("parses multiple events", async () => {
    const raw =
      "data: first\n\ndata: second\n\nevent: custom\ndata: third\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 3);
    assert.equal(events[0].data, "first");
    assert.equal(events[1].data, "second");
    assert.equal(events[2].event, "custom");
    assert.equal(events[2].data, "third");
  });

  it("handles keep-alive comments without emitting events", async () => {
    const raw = ": keep-alive\n\n";
    const events = await collectEvents(toChunks(raw));
    // A blank line after a comment does not emit because there is no data.
    assert.equal(events.length, 0);
  });

  it("handles chunked delivery across data boundaries", async () => {
    const events = await collectEvents(
      toChunks("da", "ta: spl", "it\n\n"),
    );
    assert.equal(events.length, 1);
    assert.equal(events[0].data, "split");
  });

  it("handles carriage return line endings", async () => {
    const raw = "data: crlf\r\n\r\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].data, "crlf");
  });

  it("resets event and id after dispatching", async () => {
    const raw =
      "id: 1\nevent: special\ndata: first\n\ndata: second\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 2);
    // First event has id and custom event
    assert.equal(events[0].id, "1");
    assert.equal(events[0].event, "special");
    // Second event is back to defaults
    assert.equal(events[1].id, null);
    assert.equal(events[1].event, "message");
  });

  it("does not emit event without data lines", async () => {
    const raw = "event: no-data\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 0);
  });

  it("handles data: with no space after colon", async () => {
    const raw = "data:nospace\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].data, "nospace");
  });

  it("reassembles a character that a chunk boundary splits", async () => {
    // U+2019 (three bytes) and U+1F600 (four bytes), cut at every byte.
    const text = "it\u2019s \u{1F600}";
    const frame = new TextEncoder().encode(`data: ${text}\n\n`);
    for (let cut = 1; cut < frame.length; cut += 1) {
      const events = await collectEvents(byteChunks(frame.slice(0, cut), frame.slice(cut)));
      assert.deepEqual(
        events.map((event) => event.data),
        [text],
        `cut at byte ${cut}`,
      );
    }
  });

  it("refuses a line that is not UTF-8 with the SDK's transport error", async () => {
    // A truncated three-byte sequence inside a complete line.
    const encoder = new TextEncoder();
    const line = new Uint8Array([
      ...encoder.encode("data: it"),
      0xe2,
      0x80,
      ...encoder.encode("s\n\n"),
    ]);
    await assert.rejects(
      collectEvents(byteChunks(line)),
      (error: unknown) => error instanceof TransportError && /UTF-8/.test(error.message),
    );
  });

  it("refuses a stream that ends inside a character", async () => {
    // A complete event, then the first two bytes of U+2019 and the end.
    const encoder = new TextEncoder();
    const cut = new Uint8Array([...encoder.encode("data: whole\n\ndata: it"), 0xe2, 0x80]);
    await assert.rejects(
      collectEvents(byteChunks(cut)),
      (error: unknown) => error instanceof TransportError && /UTF-8/.test(error.message),
    );
  });

  it("does not dispatch a final event without its closing blank line", async () => {
    // The SSE spec discards pending data at the end of the stream.
    const events = await collectEvents(
      toChunks("data: first\n\n", "event: last\ndata: pending\n"),
    );
    assert.deepEqual(
      events.map((event) => event.data),
      ["first"],
    );
  });

  it("parses a full event with all fields", async () => {
    const raw = "id: evt-99\nevent: agent_event\ndata: {\"type\":\"text_delta\",\"delta\":\"hi\"}\n\n";
    const events = await collectEvents(toChunks(raw));
    assert.equal(events.length, 1);
    assert.equal(events[0].id, "evt-99");
    assert.equal(events[0].event, "agent_event");
    const parsed = JSON.parse(events[0].data);
    assert.equal(parsed.type, "text_delta");
    assert.equal(parsed.delta, "hi");
  });
});

// ---------------------------------------------------------------------------
// encodeSseEvent
// ---------------------------------------------------------------------------

describe("encodeSseEvent", () => {
  it("encodes a basic message event (no event: line for default)", () => {
    const encoded = encodeSseEvent({
      id: null,
      event: "message",
      data: "hello",
    });
    assert.equal(encoded, "data: hello\n\n");
  });

  it("encodes a custom event type", () => {
    const encoded = encodeSseEvent({
      id: null,
      event: "custom",
      data: "payload",
    });
    assert.equal(encoded, "event: custom\ndata: payload\n\n");
  });

  it("encodes an event with an id", () => {
    const encoded = encodeSseEvent({
      id: "42",
      event: "message",
      data: "data",
    });
    assert.equal(encoded, "id: 42\ndata: data\n\n");
  });

  it("encodes multi-line data as separate data: lines", () => {
    const encoded = encodeSseEvent({
      id: null,
      event: "message",
      data: "line1\nline2\nline3",
    });
    assert.equal(encoded, "data: line1\ndata: line2\ndata: line3\n\n");
  });

  it("encodes all fields together", () => {
    const encoded = encodeSseEvent({
      id: "evt-1",
      event: "update",
      data: "content",
    });
    assert.equal(encoded, "id: evt-1\nevent: update\ndata: content\n\n");
  });

  it("roundtrips through encode → parse", async () => {
    const original: SseEvent = {
      id: "rt-1",
      event: "test_event",
      data: "line1\nline2",
    };
    const encoded = encodeSseEvent(original);
    const events = await collectEvents(toChunks(encoded));
    assert.equal(events.length, 1);
    assert.equal(events[0].id, original.id);
    assert.equal(events[0].event, original.event);
    assert.equal(events[0].data, original.data);
  });

  it("roundtrips a message event through encode → parse", async () => {
    const original: SseEvent = {
      id: null,
      event: "message",
      data: "simple",
    };
    const encoded = encodeSseEvent(original);
    const events = await collectEvents(toChunks(encoded));
    assert.equal(events.length, 1);
    assert.equal(events[0].event, "message");
    assert.equal(events[0].data, "simple");
  });
});
