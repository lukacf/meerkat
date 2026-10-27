import test from "node:test";
import assert from "node:assert/strict";

import {
  DEFAULT_CONSOLE_FETCH_TIMEOUT_MS,
  callConsoleRpc,
  fetchJson,
  parseSseFrames,
  queryTimeline,
  sendConsole,
  sendConsoleMultipart,
  subscribeTimelineEvents,
  uploadConsoleBlobMultipart,
} from "./network";
import { mapFramesToTimelineEntries } from "./adapters";
import type { ConsoleFrame } from "../types";
import { accessSaveFailure, accessSaveNotice } from "./errors";

// Wait until `condition` holds or fail after `timeoutMs`. The timeline
// subscription reconnects after a jittered 200-300 ms backoff, and parallel
// test files can hold a timer past any fixed margin, so the tests wait for
// what they observe rather than for a fixed time.
async function until(condition: () => boolean, label: string, timeoutMs = 5_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) assert.fail(`timed out after ${timeoutMs} ms waiting for ${label}`);
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

// How long a test watches for a reconnect the subscription must not make:
// twice the longest first backoff (250 ms plus 20% jitter).
const NO_RECONNECT_WINDOW_MS = 600;

test("fetchJson defaults console requests to a 60 second timeout with an abort reason", async () => {
  const originalFetch = globalThis.fetch;
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  let scheduledMs: number | undefined;
  let cleared = false;

  globalThis.setTimeout = ((handler: TimerHandler, timeout?: number) => {
    scheduledMs = timeout;
    return originalSetTimeout(handler, 0);
  }) as typeof setTimeout;
  globalThis.clearTimeout = ((handle?: number) => {
    cleared = true;
    return originalClearTimeout(handle);
  }) as typeof clearTimeout;
  globalThis.fetch = (async (_input, init) => {
    const signal = init?.signal;
    return new Promise<Response>((_resolve, reject) => {
      signal?.addEventListener("abort", () => reject(signal.reason));
    });
  }) as typeof fetch;

  try {
    await assert.rejects(
      fetchJson("http://127.0.0.1:7000", "/console/experience"),
      /console fetch timeout after 60 s/,
    );
    assert.equal(scheduledMs, DEFAULT_CONSOLE_FETCH_TIMEOUT_MS);
    assert.equal(cleared, true);
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
  }
});

test("console RPC requests use the configured timeout with an abort reason", async () => {
  const originalFetch = globalThis.fetch;
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  let scheduledMs: number | undefined;
  let cleared = false;

  globalThis.setTimeout = ((handler: TimerHandler, timeout?: number) => {
    scheduledMs = timeout;
    return originalSetTimeout(handler, 0);
  }) as typeof setTimeout;
  globalThis.clearTimeout = ((handle?: number) => {
    cleared = true;
    return originalClearTimeout(handle);
  }) as typeof clearTimeout;
  globalThis.fetch = (async (_input, init) => {
    const signal = init?.signal;
    return new Promise<Response>((_resolve, reject) => {
      signal?.addEventListener("abort", () => reject(signal.reason));
    });
  }) as typeof fetch;

  try {
    await assert.rejects(
      queryTimeline("http://127.0.0.1:7000", { identity: "agent-a" }, 10, 25),
      /console rpc timeout after 25 ms/,
    );
    assert.equal(scheduledMs, 25);
    assert.equal(cleared, true);
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
  }
});

test("console multipart requests use the configured timeout with an abort reason", async () => {
  const originalFetch = globalThis.fetch;
  const originalSetTimeout = globalThis.setTimeout;
  const originalClearTimeout = globalThis.clearTimeout;
  let scheduledMs: number | undefined;
  let cleared = false;

  globalThis.setTimeout = ((handler: TimerHandler, timeout?: number) => {
    scheduledMs = timeout;
    return originalSetTimeout(handler, 0);
  }) as typeof setTimeout;
  globalThis.clearTimeout = ((handle?: number) => {
    cleared = true;
    return originalClearTimeout(handle);
  }) as typeof clearTimeout;
  globalThis.fetch = (async (_input, init) => {
    const signal = init?.signal;
    return new Promise<Response>((_resolve, reject) => {
      signal?.addEventListener("abort", () => reject(signal.reason));
    });
  }) as typeof fetch;

  try {
    await assert.rejects(
      sendConsoleMultipart(
        "http://127.0.0.1:7000",
        "agent-a",
        "hello",
        [new File(["png"], "image.png", { type: "image/png" })],
        "test",
        "idem-timeout",
        "queue",
        25,
      ),
      /console multipart timeout after 25 ms/,
    );
    assert.equal(scheduledMs, 25);
    assert.equal(cleared, true);
  } finally {
    globalThis.fetch = originalFetch;
    globalThis.setTimeout = originalSetTimeout;
    globalThis.clearTimeout = originalClearTimeout;
  }
});

test("console HTTP errors expose bounded response previews", async () => {
  const originalFetch = globalThis.fetch;
  const body = `prefix-${"x".repeat(700)}-secret-tail`;
  globalThis.fetch = (async () => new Response(body, { status: 502 })) as typeof fetch;

  try {
    await assert.rejects(
      fetchJson("http://127.0.0.1:7000", "/console/experience", 60_000),
      (error: unknown) => {
        assert.ok(error instanceof Error);
        assert.match(error.message, /Request failed 502/);
        assert.match(error.message, /prefix-/);
        assert.doesNotMatch(error.message, /secret-tail/);
        return true;
      },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("timeline stream HTTP errors expose bounded previews as transport state, without replay frames", async () => {
  const originalFetch = globalThis.fetch;
  const body = `prefix-${"x".repeat(700)}-secret-tail`;
  globalThis.fetch = (async () => new Response(body, { status: 502 })) as typeof fetch;
  const frames: ConsoleFrame[] = [];
  const errors: unknown[] = [];
  const unsubscribe = subscribeTimelineEvents("http://127.0.0.1:7000", {}, (frame) => frames.push(frame), {
    onTransportState: (state) => { if (state.error) errors.push(state.error); },
  });
  try {
    await until(() => errors.length > 0, "the stream's transport error");
    assert.equal(frames.length, 0);
    const message = String((errors[0] as Error)?.message || "");
    assert.match(message, /interaction stream request failed 502/);
    assert.match(message, /prefix-/);
    assert.doesNotMatch(message, /secret-tail/);
  } finally {
    unsubscribe();
    globalThis.fetch = originalFetch;
  }
});

test("queryTimeline normalizes aggregate log frames", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async () => new Response(JSON.stringify({
    jsonrpc: "2.0",
    id: "meta/console/query_timeline:1",
    result: {
      frames: [
        {
          id: "console-frame-1",
          cursor: "console:1",
          dedupe_key: "send:runtime:agent:origin:key",
          timestamp_ms: 10,
          runtime_key: "runtime-a",
          identity: "agent-a",
          conversation_id: "agent-a",
          session_id: "session-a",
          kind: "user_input",
          status: "delivered",
          frame_version: 2,
          updated_at_ms: 11,
          payload: { content: "hello" },
          source: { kind: "send" },
          interaction_id: "turn-1",
          turn_id: "turn-1",
          run_id: "run-1",
        },
      ],
      next_cursor: "console:1",
    },
  }), {
    status: 200,
    headers: { "content-type": "application/json" },
  })) as typeof fetch;

  try {
    const result = await queryTimeline("http://127.0.0.1:7000", { identity: "agent-a" }, 10);
    assert.equal(result.available, true);
    assert.equal(result.nextCursor, "console:1");
    assert.equal(result.frames[0]?.event, "user_input");
    assert.equal(result.frames[0]?.cursor, "console:1");
    assert.equal(result.frames[0]?.status, "delivered");
    assert.equal(result.frames[0]?.frameVersion, 2);
    assert.equal(result.frames[0]?.turnId, "turn-1");
    assert.deepEqual(result.frames[0]?.data, { content: "hello" });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("queryTimeline parsed production frames suppress raw peer prompt when structured notice exists", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async () => new Response(JSON.stringify({
    jsonrpc: "2.0",
    id: "meta/console/query_timeline:1",
    result: {
      frames: [
        {
          id: "raw-run-started",
          cursor: "console:1",
          timestamp_ms: Date.parse("2026-05-27T08:00:00.000Z"),
          identity: "planner",
          kind: "run_started",
          payload: {
            prompt:
              "Peer message\n"
              + "Peer message from fugue/issue_lead/LUC-642/issue_lead:\n"
              + "Focused RED-review replan is complete.\n"
              + "Peer message",
          },
          source: { kind: "live" },
        },
        {
          id: "structured-notice",
          cursor: "console:2",
          timestamp_ms: Date.parse("2026-05-27T08:00:01.000Z"),
          identity: "planner",
          kind: "system_notice",
          payload: {
            message: {
              role: "system_notice",
              kind: "comms",
              body: "Focused RED-review replan is complete.",
              blocks: [{
                type: "comms",
                kind: "message",
                direction: "incoming",
                peer: {
                  id: "fugue/issue_lead/LUC-642/issue_lead",
                  display_name: "fugue/issue_lead/LUC-642/issue_lead",
                },
                request_id: "request-1",
                content: [{
                  type: "text",
                  text: "Focused RED-review replan is complete.",
                }],
              }],
            },
          },
          source: { kind: "live" },
        },
      ],
      next_cursor: "console:2",
    },
  }))) as typeof fetch;

  try {
    const page = await queryTimeline("http://127.0.0.1:7000", { identity: "planner" });
    const entries = mapFramesToTimelineEntries(
      {
        agent_id: "planner",
        member_id: "planner",
        label: "Planner",
        kind: "identity",
      },
      page.frames,
      { renderInteractionStartsAsUser: true },
    );

    assert.equal(entries.length, 1);
    assert.equal(entries[0]?.identity.id, "comms");
    assert.equal(entries.some((entry) => (
      entry.identity.id === "user"
      && "text" in entry
      && entry.text.includes("Peer message from fugue/issue_lead")
    )), false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("queryTimeline normalizes replayable frame update markers", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async () => new Response(JSON.stringify({
    jsonrpc: "2.0",
    id: "meta/console/query_timeline:1",
    result: {
      frames: [
        {
          id: "console-frame-update-1",
          cursor: "console:2",
          dedupe_key: "frame-update:console-frame-1:2",
          timestamp_ms: 12,
          runtime_key: "runtime-a",
          identity: "agent-a",
          conversation_id: "agent-a",
          session_id: "session-a",
          kind: "frame_updated",
          status: "delivered",
          frame_version: 1,
          payload: {
            frame: {
              id: "console-frame-1",
              cursor: "console:1",
              dedupe_key: "send:runtime:agent:origin:key",
              timestamp_ms: 10,
              runtime_key: "runtime-a",
              identity: "agent-a",
              kind: "user_input",
              status: "delivered",
              frame_version: 2,
              updated_at_ms: 12,
              payload: { content: "hello" },
              source: { kind: "send" },
              interaction_id: "turn-1",
            },
          },
          source: { kind: "synthetic" },
          interaction_id: "turn-1",
          parent_frame_id: "console-frame-1",
          caused_by_frame_id: "console-frame-1",
        },
      ],
      next_cursor: "console:2",
    },
  }), {
    status: 200,
    headers: { "content-type": "application/json" },
  })) as typeof fetch;

  try {
    const result = await queryTimeline("http://127.0.0.1:7000", { identity: "agent-a", after: "console:1" }, 10);
    assert.equal(result.frames[0]?.event, "frame_updated");
    assert.equal(result.frames[0]?.cursor, "console:2");
    const updated = (result.frames[0]?.data as { frame?: { status?: string; cursor?: string } }).frame;
    assert.equal(updated?.status, "delivered");
    assert.equal(updated?.cursor, "console:1");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("queryTimeline exposes typed replay-unavailable RPC errors", async () => {
  const originalFetch = globalThis.fetch;

  globalThis.fetch = (async () => new Response(JSON.stringify({
    jsonrpc: "2.0",
    id: "meta/console/query_timeline:1",
    error: {
      code: -32013,
      message: "query_timeline failed: replay unavailable",
      data: {
        error: "replay_unavailable",
        stream: "timeline",
        requested_cursor: "console:500",
        latest_cursor: "console:42",
      },
    },
  }), {
    status: 200,
    headers: { "content-type": "application/json" },
  })) as typeof fetch;

  try {
    await assert.rejects(
      queryTimeline("http://127.0.0.1:7000", { after: "console:500" }, 10),
      (error: unknown) => {
        const replay = error as Error & {
          replayError?: { stream?: string; requested_last_event_id?: string; latest_event_id?: string };
          timelineReplayUnavailable?: boolean;
        };
        assert.equal(replay.timelineReplayUnavailable, true);
        assert.equal(replay.replayError?.stream, "timeline");
        assert.equal(replay.replayError?.requested_last_event_id, "console:500");
        assert.equal(replay.replayError?.latest_event_id, "console:42");
        return true;
      },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("sendConsole posts idempotent console send RPC", async () => {
  const originalFetch = globalThis.fetch;
  let body = "";

  globalThis.fetch = (async (_input, init) => {
    body = typeof init?.body === "string" ? init.body : "";
    return new Response(JSON.stringify({
      jsonrpc: "2.0",
      id: "meta/console/send:1",
      result: {
        interaction_id: "turn-1",
        identity: "agent-a",
        input_frame_id: "console-frame-1",
        cursor: "console:1",
        status: "accepted",
      },
    }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;

  try {
    const result = await sendConsole(
      "http://127.0.0.1:7000",
      "agent-a",
      "hello",
      "console:panel",
      "idem-1",
    );
    assert.match(body, /"method":"meta\/console\/send"/);
    assert.match(body, /"idempotency_key":"idem-1"/);
    assert.equal(result.input_frame_id, "console-frame-1");
    assert.equal(result.cursor, "console:1");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("sendConsoleMultipart posts identity multipart sends with upload placeholders", async () => {
  const originalFetch = globalThis.fetch;
  let payload = "";
  let hasFilePart = false;

  globalThis.fetch = (async (_input, init) => {
    const form = init?.body as FormData;
    payload = String(form.get("payload") || "");
    hasFilePart = Boolean(form.get("file:upload-test-0"));
    return new Response(JSON.stringify({
      jsonrpc: "2.0",
      id: "meta/console/send:1",
      result: {
        interaction_id: "turn-1",
        identity: "agent-a",
        input_frame_id: "console-frame-1",
        cursor: "console:1",
        status: "accepted",
      },
    }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;

  const originalNow = Date.now;
  try {
    Date.now = () => Number.parseInt("test", 36);
    const result = await sendConsoleMultipart(
      "http://127.0.0.1:7000",
      "agent-a",
      "describe this",
      [new File(["png"], "badge.png", { type: "image/png" })],
      "console:panel",
      "idem-1",
    );
    assert.match(payload, /"method":"meta\/console\/send"/);
    assert.match(payload, /"identity":"agent-a"/);
    assert.match(payload, /"idempotency_key":"idem-1"/);
    assert.match(payload, /"upload_id":"upload-test-0"/);
    assert.equal(hasFilePart, true);
    assert.equal(result.input_frame_id, "console-frame-1");
    assert.equal(result.cursor, "console:1");
  } finally {
    Date.now = originalNow;
    globalThis.fetch = originalFetch;
  }
});

test("sendConsoleMultipart preserves structured content before upload placeholders", async () => {
  const originalFetch = globalThis.fetch;
  let payload: Record<string, unknown> | null = null;

  globalThis.fetch = (async (_input, init) => {
    const form = init?.body as FormData;
    payload = JSON.parse(String(form.get("payload") || "{}")) as Record<string, unknown>;
    return new Response(JSON.stringify({
      jsonrpc: "2.0",
      id: "meta/console/send:1",
      result: {
        interaction_id: "turn-structured",
        identity: "agent-a",
        status: "accepted",
      },
    }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;

  const originalNow = Date.now;
  try {
    Date.now = () => Number.parseInt("test", 36);
    await sendConsoleMultipart(
      "http://127.0.0.1:7000",
      "agent-a",
      [
        { type: "text", text: "Keep this block" },
        { type: "tool_context", id: "ctx-1" },
      ],
      [new File(["png"], "badge.png", { type: "image/png" })],
      "console:panel",
      "idem-structured",
    );

    const params = payload?.params as { content?: Array<Record<string, unknown>> } | undefined;
    assert.deepEqual(params?.content, [
      { type: "text", text: "Keep this block" },
      { type: "tool_context", id: "ctx-1" },
      {
        type: "image_upload",
        upload_id: "upload-test-0",
        media_type: "image/png",
        alt: "badge.png",
      },
    ]);
  } finally {
    Date.now = originalNow;
    globalThis.fetch = originalFetch;
  }
});

test("uploadConsoleBlobMultipart posts blob upload multipart RPC with one file", async () => {
  const originalFetch = globalThis.fetch;
  let payload: Record<string, unknown> | null = null;
  let hasFilePart = false;

  globalThis.fetch = (async (_input, init) => {
    const form = init?.body as FormData;
    payload = JSON.parse(String(form.get("payload") || "{}")) as Record<string, unknown>;
    hasFilePart = Boolean(form.get("file:upload-test-0"));
    return new Response(JSON.stringify({
      jsonrpc: "2.0",
      id: "meta/blob/upload:1",
      result: {
        blob_id: "blob-1",
        media_type: "image/png",
        size: 3,
      },
    }), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;

  const originalNow = Date.now;
  try {
    Date.now = () => Number.parseInt("test", 36);
    const result = await uploadConsoleBlobMultipart("http://127.0.0.1:7000", {
      file: new File(["png"], "badge.png", { type: "image/png" }),
    });
    assert.equal(hasFilePart, true);
    assert.equal(payload?.method, "meta/blob/upload");
    assert.deepEqual((payload?.params as { upload?: Record<string, unknown> }).upload, {
      type: "image_upload",
      upload_id: "upload-test-0",
      media_type: "image/png",
      alt: "badge.png",
    });
    assert.deepEqual(result, { blob_id: "blob-1", url: undefined });
  } finally {
    Date.now = originalNow;
    globalThis.fetch = originalFetch;
  }
});

test("parseSseFrames unwraps aggregate timeline frames", () => {
  const frames = parseSseFrames([
    "id: console:4",
    "event: console_frame",
    'data: {"type":"console_frame","frame":{"id":"console-frame-4","cursor":"console:4","dedupe_key":"event-4","timestamp_ms":4,"runtime_key":"runtime-a","identity":"agent-a","kind":"user_input","status":"accepted","frame_version":1,"payload":{"content":"hello"},"source":{"kind":"session_history"},"interaction_id":"turn-4"}}',
    "",
  ].join("\n"));

  assert.equal(frames.length, 1);
  assert.equal(frames[0]?.id, "console-frame-4");
  assert.equal(frames[0]?.event, "user_input");
  assert.equal(frames[0]?.cursor, "console:4");
  assert.equal(frames[0]?.frameVersion, 1);
  assert.equal(frames[0]?.sourceKind, "session_history");
  assert.deepEqual(frames[0]?.data, { content: "hello" });
});

test("parseSseFrames unwraps aggregate frame update events", () => {
  const frames = parseSseFrames([
    "id: console:5",
    "event: frame_updated",
    'data: {"type":"console_frame","frame":{"id":"console-frame-update-5","cursor":"console:5","dedupe_key":"frame-update:console-frame-4:2","timestamp_ms":5,"runtime_key":"runtime-a","identity":"agent-a","kind":"frame_updated","status":"delivered","frame_version":1,"payload":{"frame":{"id":"console-frame-4","cursor":"console:4","dedupe_key":"event-4","timestamp_ms":4,"runtime_key":"runtime-a","identity":"agent-a","kind":"user_input","status":"delivered","frame_version":2,"updated_at_ms":5,"payload":{"content":"hello"},"source":{"kind":"send"},"interaction_id":"turn-4"}},"source":{"kind":"synthetic"},"interaction_id":"turn-4","parent_frame_id":"console-frame-4"}}',
    "",
  ].join("\n"));

  assert.equal(frames.length, 1);
  assert.equal(frames[0]?.id, "console-frame-update-5");
  assert.equal(frames[0]?.event, "frame_updated");
  assert.equal(frames[0]?.cursor, "console:5");
  const updated = (frames[0]?.data as { frame?: { id?: string; status?: string } }).frame;
  assert.equal(updated?.id, "console-frame-4");
  assert.equal(updated?.status, "delivered");
});

test("subscribeTimelineEvents reconnects with the latest aggregate cursor", async () => {
  const originalFetch = globalThis.fetch;
  const calls: string[] = [];
  const lastEventIds: Array<string | undefined> = [];
  const seen: string[] = [];
  let unsubscribe = () => {};

  globalThis.fetch = (async (input, init) => {
    const url = String(input);
    calls.push(url);
    lastEventIds.push((init?.headers as Record<string, string> | undefined)?.["Last-Event-ID"]);
    if (calls.length === 1) {
      return new Response([
        "id: console:10",
        "event: snapshot_complete",
        'data: {"type":"snapshot_complete","cursor":"console:10"}',
        "",
      ].join("\n"), {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    }
    return new Response([
      "id: console:11",
      "event: console_frame",
      'data: {"type":"console_frame","frame":{"id":"console-frame-11","cursor":"console:11","dedupe_key":"event-11","timestamp_ms":11,"runtime_key":"runtime-a","identity":"agent-a","kind":"text_complete","status":"completed","frame_version":1,"payload":{"text":"after reconnect"},"source":{"kind":"console_event"}}}',
      "",
    ].join("\n"), {
      status: 200,
      headers: { "content-type": "text/event-stream" },
    });
  }) as typeof fetch;

  try {
    unsubscribe = subscribeTimelineEvents("http://127.0.0.1:7000", {}, (frame) => {
      seen.push(frame.event);
      if (frame.cursor === "console:11") {
        unsubscribe();
      }
    });
    await until(() => seen.length === 2, "the frame after the reconnect");
    assert.equal(calls[0], "http://127.0.0.1:7000/console/timeline/stream");
    assert.equal(calls[1], "http://127.0.0.1:7000/console/timeline/stream");
    assert.deepEqual(lastEventIds, [undefined, "console:10"]);
    assert.deepEqual(seen, ["snapshot_complete", "text_complete"]);
  } finally {
    unsubscribe();
    globalThis.fetch = originalFetch;
  }
});

test("subscribeTimelineEvents reports stale cursor without adopting an unaccepted latest cursor", async () => {
  const originalFetch = globalThis.fetch;
  const calls: string[] = [];
  const lastEventIds: Array<string | undefined> = [];
  const seen: string[] = [];
  let unsubscribe = () => {};

  globalThis.fetch = (async (input, init) => {
    const url = String(input);
    calls.push(url);
    lastEventIds.push((init?.headers as Record<string, string> | undefined)?.["Last-Event-ID"]);
    if (calls.length === 1) {
      return new Response(JSON.stringify({
        error: "replay_unavailable",
        requested_cursor: "bad",
        latest_cursor: "console:99",
      }), {
        status: 409,
        headers: { "content-type": "application/json" },
      });
    }
    return new Response([
      "id: console:100",
      "event: console_frame",
      'data: {"type":"console_frame","frame":{"id":"console-frame-100","cursor":"console:100","dedupe_key":"event-100","timestamp_ms":100,"runtime_key":"runtime-a","identity":"agent-a","kind":"text_complete","status":"completed","frame_version":1,"payload":{"text":"after stale cursor recovery"},"source":{"kind":"console_event"}}}',
      "",
    ].join("\n"), {
      status: 200,
      headers: { "content-type": "text/event-stream" },
    });
  }) as typeof fetch;

  try {
    unsubscribe = subscribeTimelineEvents(
      "http://127.0.0.1:7000",
      { after: "bad" },
      (frame) => {
        seen.push(frame.event);
        if (frame.cursor === "console:100") {
          unsubscribe();
        }
      },
    );
    await until(() => seen.length === 1, "the replay_unavailable report");
    await new Promise((resolve) => setTimeout(resolve, NO_RECONNECT_WINDOW_MS));
    assert.equal(calls[0], "http://127.0.0.1:7000/console/timeline/stream");
    assert.equal(calls.length, 1);
    assert.deepEqual(lastEventIds, ["bad"]);
    assert.deepEqual(seen, ["replay_unavailable"]);
  } finally {
    unsubscribe();
    globalThis.fetch = originalFetch;
  }
});

test("subscribeTimelineEvents delegates in-stream gap recovery without skipping history", async () => {
  const originalFetch = globalThis.fetch;
  const calls: string[] = [];
  const lastEventIds: Array<string | undefined> = [];
  const seen: string[] = [];
  let unsubscribe = () => {};

  globalThis.fetch = (async (input, init) => {
    const url = String(input);
    calls.push(url);
    lastEventIds.push((init?.headers as Record<string, string> | undefined)?.["Last-Event-ID"]);
    if (calls.length === 1) {
      return new Response([
        "event: replay_unavailable",
        'data: {"type":"replay_unavailable","requested_cursor":"lagged:4","latest_cursor":"console:99"}',
        "",
      ].join("\n"), {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    }
    return new Response([
      "id: console:100",
      "event: console_frame",
      'data: {"type":"console_frame","frame":{"id":"console-frame-100","cursor":"console:100","dedupe_key":"event-100","timestamp_ms":100,"runtime_key":"runtime-a","identity":"agent-a","kind":"text_complete","status":"completed","frame_version":1,"payload":{"text":"after stream replay gap"},"source":{"kind":"console_event"}}}',
      "",
    ].join("\n"), {
      status: 200,
      headers: { "content-type": "text/event-stream" },
    });
  }) as typeof fetch;

  try {
    unsubscribe = subscribeTimelineEvents(
      "http://127.0.0.1:7000",
      {},
      (frame) => {
        seen.push(frame.event);
        if (frame.cursor === "console:100") {
          unsubscribe();
        }
      },
    );
    await until(() => seen.length === 1, "the replay_unavailable report");
    await new Promise((resolve) => setTimeout(resolve, NO_RECONNECT_WINDOW_MS));
    assert.equal(calls[0], "http://127.0.0.1:7000/console/timeline/stream");
    assert.equal(calls.length, 1);
    assert.deepEqual(lastEventIds, [undefined]);
    assert.deepEqual(seen, ["replay_unavailable"]);
  } finally {
    unsubscribe();
    globalThis.fetch = originalFetch;
  }
});

test("subscribeTimelineEvents reconnects from the last delivered cursor after stream end", async () => {
  const originalFetch = globalThis.fetch;
  const calls: string[] = [];
  const lastEventIds: Array<string | undefined> = [];
  const seen: string[] = [];
  let unsubscribe = () => {};

  globalThis.fetch = (async (input, init) => {
    const url = String(input);
    calls.push(url);
    lastEventIds.push((init?.headers as Record<string, string> | undefined)?.["Last-Event-ID"]);
    if (calls.length === 1) {
      return new Response([
        "id: console:41",
        "event: console_frame",
        'data: {"type":"console_frame","frame":{"id":"console-frame-41","cursor":"console:41","dedupe_key":"event-41","timestamp_ms":41,"runtime_key":"runtime-a","identity":"agent-a","kind":"text_complete","status":"completed","frame_version":1,"payload":{"text":"before lag"},"source":{"kind":"console_event"}}}',
        "",
      ].join("\n"), {
        status: 200,
        headers: { "content-type": "text/event-stream" },
      });
    }
    return new Response([
      "id: console:42",
      "event: console_frame",
      'data: {"type":"console_frame","frame":{"id":"console-frame-42","cursor":"console:42","dedupe_key":"event-42","timestamp_ms":42,"runtime_key":"runtime-a","identity":"agent-a","kind":"text_complete","status":"completed","frame_version":1,"payload":{"text":"replayed after reconnect"},"source":{"kind":"console_event"}}}',
      "",
    ].join("\n"), {
      status: 200,
      headers: { "content-type": "text/event-stream" },
    });
  }) as typeof fetch;

  try {
    unsubscribe = subscribeTimelineEvents(
      "http://127.0.0.1:7000",
      {},
      (frame) => {
        seen.push(`${frame.event}:${frame.cursor ?? ""}`);
        if (frame.cursor === "console:42") {
          unsubscribe();
        }
      },
    );
    await until(() => seen.length === 2, "the frame replayed after the reconnect");
    assert.equal(calls[0], "http://127.0.0.1:7000/console/timeline/stream");
    assert.equal(calls[1], "http://127.0.0.1:7000/console/timeline/stream");
    assert.deepEqual(lastEventIds, [undefined, "console:41"]);
    assert.deepEqual(seen, ["text_complete:console:41", "text_complete:console:42"]);
  } finally {
    unsubscribe();
    globalThis.fetch = originalFetch;
  }
});

import { classifyConsoleSendFailure } from "../../../packages/console-core/src/send-attempt";

// The typed 401 body the gateway answers on both console send doors
// (crates/meerkat-meta tests/console_route_auth.rs pins the server side).
const UNAUTHENTICATED_BODY = JSON.stringify({
  jsonrpc: "2.0",
  id: null,
  error: {
    code: -32600,
    message: "unauthorized: console rpc requires a valid auth token",
    data: { kind: "unauthenticated", http_status: 401 },
  },
});

test("an unauthenticated console send is a typed rejection on the JSON and multipart doors", async () => {
  const originalFetch = globalThis.fetch;
  globalThis.fetch = (async () => new Response(UNAUTHENTICATED_BODY, {
    status: 401,
    headers: { "content-type": "application/json" },
  })) as typeof fetch;
  try {
    for (const send of [
      () => sendConsole("http://127.0.0.1:7000", "domain:calendar", "hello", "console:p", "idem-401", "queue", 60_000),
      () => sendConsoleMultipart(
        "http://127.0.0.1:7000", "domain:calendar", "hello",
        [new File([new Uint8Array([1])], "a.png", { type: "image/png" })],
        "console:p", "idem-401-mp", "steer", 60_000,
      ),
    ]) {
      await assert.rejects(send(), (error: unknown) => {
        const typed = error as { httpStatus?: number; responseRpcError?: { data?: { kind?: string } } };
        assert.equal(typed.httpStatus, 401);
        assert.equal(typed.responseRpcError?.data?.kind, "unauthenticated");
        const failure = classifyConsoleSendFailure(error);
        assert.equal(failure.state, "definitely-rejected");
        assert.equal(failure.kind, "unauthenticated");
        assert.match(failure.message, /^Sign-in required \(401\)\./);
        return true;
      });
    }
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("a proxy's bare 401 or 403 page is still a typed rejection", async () => {
  const originalFetch = globalThis.fetch;
  try {
    for (const [status, kind] of [[401, "unauthenticated"], [403, "access_denied"]] as const) {
      globalThis.fetch = (async () => new Response("<html>denied</html>", {
        status, headers: { "content-type": "text/html" },
      })) as typeof fetch;
      await assert.rejects(
        sendConsole("http://127.0.0.1:7000", "domain:calendar", "hi", "console:p", `idem-${status}`, "queue", 60_000),
        (error: unknown) => {
          const failure = classifyConsoleSendFailure(error);
          assert.equal(failure.state, "definitely-rejected");
          assert.equal(failure.kind, kind);
          return true;
        },
      );
    }
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("transport failures are typed and stay reconcilable", async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = (async () => { throw new TypeError("Failed to fetch"); }) as typeof fetch;
    await assert.rejects(
      sendConsole("http://127.0.0.1:7000", "domain:calendar", "hi", "console:p", "idem-down", "queue", 60_000),
      (error: unknown) => {
        assert.ok(error instanceof TypeError, "the fetch error itself is kept");
        const failure = classifyConsoleSendFailure(error);
        assert.equal(failure.state, "outcome-unknown");
        assert.equal(failure.kind, "connection_failed");
        assert.match(failure.message, /^The connection failed before the gateway answered \(Failed to fetch\)\. The message may already have been accepted/);
        return true;
      },
    );

    globalThis.fetch = (async () => new Response("<html>login</html>", {
      status: 200, headers: { "content-type": "text/html" },
    })) as typeof fetch;
    await assert.rejects(
      sendConsole("http://127.0.0.1:7000", "domain:calendar", "hi", "console:p", "idem-html", "queue", 60_000),
      (error: unknown) => {
        const failure = classifyConsoleSendFailure(error);
        assert.equal(failure.state, "outcome-unknown");
        assert.equal(failure.kind, "invalid_response");
        assert.match(failure.message, /non-JSON response \(HTTP 200, text\/html\)/);
        return true;
      },
    );

    globalThis.fetch = (async () => new Response("bad gateway", { status: 502 })) as typeof fetch;
    await assert.rejects(
      sendConsoleMultipart(
        "http://127.0.0.1:7000", "domain:calendar", "hi",
        [new File([new Uint8Array([1])], "a.png", { type: "image/png" })],
        "console:p", "idem-502", "queue", 60_000,
      ),
      (error: unknown) => {
        const failure = classifyConsoleSendFailure(error);
        assert.equal(failure.state, "outcome-unknown");
        assert.equal(failure.kind, "gateway_error");
        assert.match(failure.message, /^Gateway error \(HTTP 502\)\./);
        return true;
      },
    );

    globalThis.fetch = (async () => new Response(JSON.stringify({
      jsonrpc: "2.0", id: 1, error: { code: -32030, message: "access denied: agent.send", data: { kind: "access_denied" } },
    }), { status: 200, headers: { "content-type": "application/json" } })) as typeof fetch;
    await assert.rejects(
      sendConsoleMultipart(
        "http://127.0.0.1:7000", "domain:calendar", "hi",
        [new File([new Uint8Array([1])], "a.png", { type: "image/png" })],
        "console:p", "idem-denied", "queue", 60_000,
      ),
      (error: unknown) => {
        const failure = classifyConsoleSendFailure(error);
        assert.equal(failure.state, "definitely-rejected", "multipart refusals are typed like JSON ones");
        assert.equal(failure.kind, "access_denied");
        return true;
      },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("access RPC preserves structured owner evidence and requires exact code and kind", async () => {
  const originalFetch = globalThis.fetch;
  const cases = [
    { error: { code: -32004, message: "private unavailable detail", data: { kind: "access_mutation_unavailable" } }, expected: "unavailable" },
    { error: { code: -32000, message: "private unavailable detail", data: { kind: "access_mutation_unavailable" } }, expected: "failed" },
    { error: { code: -32004, message: "private unavailable detail", data: { kind: "unknown_owner_error" } }, expected: "failed" },
    { error: { code: -32004, message: "access_mutation_unavailable" }, expected: "failed" },
    { error: { code: -32004, message: "private unavailable detail", data: null }, expected: "failed" },
    { error: { code: -32602, message: "private validation detail", data: { kind: "invalid_access_config" } }, expected: "invalid" },
    { error: { code: -32602, message: "invalid access config: missing field `config`" }, expected: "unavailable" },
    { error: { code: -32004, message: "private validation detail", data: { kind: "invalid_access_config" } }, expected: "failed" },
  ];
  try {
    for (const row of cases) {
      globalThis.fetch = (async () => new Response(JSON.stringify({ jsonrpc: "2.0", id: "access-test", error: row.error }), { status: 200 })) as typeof fetch;
      await assert.rejects(callConsoleRpc("http://console.test", "meta/access/set", { checked_v1: {} }), (error: unknown) => {
        assert.deepEqual((error as { rpcError?: unknown }).rpcError, row.error, "the transport must retain the owner's structured evidence");
        assert.equal(accessSaveFailure(error).kind, row.expected, "neither a coincidental code nor message text establishes the outcome");
        assert.doesNotMatch(accessSaveNotice(accessSaveFailure(error)), /private|missing field/);
        return true;
      });
    }
  } finally { globalThis.fetch = originalFetch; }
});

test("ordinary console interaction code -32004 retains its existing send disposition", async () => {
  const originalFetch = globalThis.fetch;
  const body = { code: -32004, message: "ordinary interaction unavailable" };
  globalThis.fetch = (async () => new Response(JSON.stringify({ jsonrpc: "2.0", id: "send-test", error: body }), { status: 200 })) as typeof fetch;
  try {
    await assert.rejects(sendConsole("http://console.test", "worker", "hello", "console:p", "idem-unavailable"), (error: unknown) => {
      assert.deepEqual((error as { rpcError?: unknown }).rpcError, body);
      assert.equal((error as Error).message, "meta/console/send RPC error -32004: ordinary interaction unavailable");
      const failure = classifyConsoleSendFailure(error);
      assert.equal(failure.state, "outcome-unknown");
      assert.equal(failure.kind, "refused");
      assert.equal(accessSaveFailure(error).kind, "failed");
      return true;
    });
  } finally { globalThis.fetch = originalFetch; }
});
