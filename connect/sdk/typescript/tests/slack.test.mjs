// Local protocol tests only. No Slack account, token, OAuth or external message.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import { SlackOperations, SlackError, SlackCancelled } from "@rkat/toolkit/connectors/slack";

// Existing Toolkit Gmail fixture lifecycle, reused without altering its owner joins.
function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
async function bounded(promise, label, milliseconds = 5_000) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`fixture deadline: ${label}`)), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
}
function reply(response, value, { status = 200, headers = {}, raw } = {}) {
  const body = raw ?? Buffer.from(JSON.stringify(value));
  response.writeHead(status, {
    "Content-Type": "application/json", "Content-Length": String(body.length),
    Connection: "close", ...headers,
  });
  response.end(body);
}

class Server {
  requests = [];
  tasks = [];
  sockets = new Set();
  socketClosures = [];
  releases = [];
  joined = false;
  closing = false;
  accepted = deferred();
  constructor(handler) {
    this.server = http.createServer((request, response) => {
      // Retain every actual handler immediately, including requests whose bodies fail.
      const task = (async () => {
        const chunks = [];
        for await (const chunk of request) chunks.push(chunk);
        const item = { method: request.method, target: request.url, headers: request.headers,
          body: Buffer.concat(chunks).toString("utf8") };
        this.requests.push(item);
        await handler(item, response);
      })();
      // Observe rejection immediately, then rethrow all errors after joining every owner.
      this.tasks.push(task.then(() => null, (error) => error));
    });
    this.server.on("connection", (socket) => {
      this.sockets.add(socket);
      this.socketClosures.push(new Promise((resolve) => {
        socket.once("close", () => { this.sockets.delete(socket); resolve(); });
      }));
      socket.on("error", () => {}); // Disconnect is an intentional input in several cases.
      this.accepted.resolve();
      if (this.closing) socket.destroy();
    });
  }
  async start() {
    this.server.listen(0, "127.0.0.1");
    await once(this.server, "listening");
    this.url = `http://127.0.0.1:${this.server.address().port}/`;
    return this;
  }
  async close() {
    this.closing = true;
    const errors = [];
    // Stop admission before waiting. All previously accepted sockets are already retained.
    const closed = new Promise((resolve, reject) => this.server.close((error) => error ? reject(error) : resolve()));
    for (const release of this.releases) release.resolve();
    for (const socket of this.sockets) socket.destroy();
    try { await bounded(closed, "listener and socket retirement"); } catch (error) { errors.push(error); }
    try { await bounded(Promise.all(this.socketClosures), "accepted socket joins"); }
    catch (error) { errors.push(error); }
    try {
      const outcomes = await bounded(Promise.all(this.tasks), "handler joins");
      errors.push(...outcomes.filter((error) => error !== null));
    } catch (error) { errors.push(error); }
    this.joined = errors.length === 0 && this.sockets.size === 0;
    if (errors.length) throw new AggregateError(errors, "fixture cleanup failures");
  }
}

async function fixture(handler, body) {
  const server = await new Server(handler).start();
  let failure;
  try { await body(server); } catch (error) { failure = error; }
  try { await server.close(); } catch (error) {
    throw new AggregateError(failure ? [failure, error] : [error], "fixture body and cleanup");
  }
  if (failure) throw failure;
  assert.equal(server.joined, true);
  return server;
}

function selected(server, account = "first", prefix = "") {
  return { baseUrl: server.url + prefix, fetch: (request) => {
    assert.equal(request.redirect, "manual");
    assert.equal(request.credentials, "omit");
    assert.equal(request.cache, "no-store");
    const headers = new Headers(request.headers);
    headers.set("Authorization", `Bearer fixture-${account}`);
    return fetch(new Request(request, { headers }));
  } };
}
function failure(kind, status = null, code = null, retry = null) {
  return (error) => {
    assert.ok(error instanceof SlackError);
    assert.equal(error.kind, kind);
    assert.equal(error.statusCode, status);
    assert.equal(error.providerCode, code);
    assert.equal(error.retryAfterSeconds, retry);
    assert.equal(error.cause, undefined);
    return true;
  };
}
const EMPTY = { ok: true, messages: [] };

test("Slack root and subpath select the same exported implementation", async () => {
  const root = await import("@rkat/toolkit");
  assert.equal(root.SlackOperations, SlackOperations);
  assert.equal(root.SlackError, SlackError);
  if (process.env.SLACK_EXPECTED_MODULE) {
    assert.equal(import.meta.resolve("@rkat/toolkit/connectors/slack"), process.env.SLACK_EXPECTED_MODULE);
  }
});

test("history preserves exact timestamps, duplicates, nested data and explicit cursor continuation", async () => {
  const value = { ok: true, messages: [{ ts: "000123.000000000001", text: "first", blocks: [null, { keep: true }] },
    { ts: "122.000001", text: "second" }, { ts: "122.000001", text: "duplicate" }], has_more: true,
    response_metadata: { next_cursor: "opaque+/= token", future: null }, future: [false, "kök"] };
  await fixture((_request, response) => reply(response, value), async (server) => {
    const operations = new SlackOperations(selected(server, "selected", "proxy/"));
    const options = { oldest: "000120.000000000001", latest: "123.999999999999", limit: 7,
      inclusive: true, includeAllMetadata: true };
    const page = await operations.historyPage("C012ABC", options);
    assert.deepEqual(page, value);
    assert.equal(server.requests.length, 1);
    let url = new URL(server.requests[0].target, server.url);
    assert.equal(url.pathname, "/proxy/api/conversations.history");
    assert.deepEqual(Object.fromEntries(url.searchParams), { channel: "C012ABC", limit: "7", inclusive: "true",
      include_all_metadata: "true", oldest: options.oldest, latest: options.latest });
    assert.equal(server.requests[0].headers["accept-encoding"], "identity");
    assert.equal(server.requests[0].headers.authorization, "Bearer fixture-selected");
    await operations.historyPage("C012ABC", { cursor: page.response_metadata.next_cursor });
    assert.equal(server.requests.length, 2, "same returned cursor never causes an automatic loop");
    url = new URL(server.requests[1].target, server.url);
    assert.equal(url.searchParams.get("cursor"), "opaque+/= token");
    assert.equal(url.searchParams.get("limit"), "15");
    assert.equal(url.searchParams.get("inclusive"), "false");
    assert.ok(server.requests.every((r) => r.method === "GET" && r.body === ""));
  });
});

test("replies addresses one explicit thread without rounding or discovering other threads", async () => {
  await fixture((_request, response) => reply(response, { ...EMPTY, messages: [{ ts: "999.000000000000001" }] }), async (server) => {
    const operations = new SlackOperations(selected(server));
    const page = await operations.repliesPage("G01", "1234567890.000000000000001", { oldest: "1", latest: "9999", limit: 999 });
    assert.equal(page.messages[0].ts, "999.000000000000001");
    const url = new URL(server.requests[0].target, server.url);
    assert.equal(url.pathname, "/api/conversations.replies");
    assert.equal(url.searchParams.get("ts"), "1234567890.000000000000001");
    assert.equal(server.requests.length, 1);
  });
});

test("empty, absent and null continuation and absent has_more remain distinct", async () => {
  const values = [EMPTY, { ...EMPTY, has_more: true }, { ...EMPTY, response_metadata: {} },
    { ...EMPTY, response_metadata: { next_cursor: "" } }, { ...EMPTY, response_metadata: { next_cursor: null } }];
  let position = 0;
  await fixture((_request, response) => reply(response, values[position++]), async (server) => {
    const operations = new SlackOperations(selected(server));
    for (const value of values) assert.deepEqual(await operations.historyPage("C1"), value);
    assert.equal(server.requests.length, values.length);
  });
});

test("a later page failure stays a failure without erasing the earlier page", async () => {
  await fixture((request, response) => request.target.includes("cursor=")
    ? reply(response, { ok: false, error: "invalid_cursor" })
    : reply(response, { ...EMPTY, response_metadata: { next_cursor: "expired-next" } }), async (server) => {
    const operations = new SlackOperations(selected(server));
    const page = await operations.historyPage("C1");
    await assert.rejects(operations.historyPage("C1", { cursor: page.response_metadata.next_cursor }),
      failure("provider", 200, "invalid_cursor"));
    assert.equal(server.requests.length, 2);
  });
});

test("two host-selected accounts remain separate despite mutation of context objects", async () => {
  await fixture((request, response) => reply(response, { ...EMPTY, observed_account: request.headers.authorization }), async (server) => {
    const a = selected(server, "a"); const b = selected(server, "b");
    const operationsA = new SlackOperations(a); const operationsB = new SlackOperations(b);
    a.baseUrl = "http://invalid.invalid/"; a.fetch = () => assert.fail("replacement transport called");
    const result = await Promise.all([operationsA.historyPage("C1"), operationsB.historyPage("C1")]);
    assert.deepEqual(result.map((page) => page.observed_account), ["Bearer fixture-a", "Bearer fixture-b"]);
    assert.equal(server.requests.length, 2);
  });
});

test("argument profile rejects malformed requests before transport", async () => {
  let entered = 0;
  const operations = new SlackOperations({ baseUrl: "https://slack.com/", fetch: () => { entered++; throw new Error(); } });
  for (const channel of ["", "C1\n", "C1\r", "C1\u2028", "../C1", "#general", "a".repeat(129), null, 123]) {
    await assert.rejects(operations.historyPage(channel), TypeError);
  }
  for (const ts of ["", "1\n", "1\u2029", "1e6", "-1", ".1", "1.", "1/2", "1".repeat(65), 123, null]) {
    await assert.rejects(operations.repliesPage("C1", ts), TypeError);
    await assert.rejects(operations.historyPage("C1", { oldest: ts }), TypeError);
  }
  for (const options of [{ limit: true }, { limit: 0 }, { limit: 1000 }, { limit: 1.2 },
    { limit: null }, { inclusive: 1 }, { includeAllMetadata: "true" }, { cursor: "" },
    { cursor: "a\n" }, { cursor: "\ud800" }, { cursor: "é".repeat(4097) }, { signal: {} }]) {
    await assert.rejects(operations.historyPage("C1", options), TypeError);
  }
  assert.equal(entered, 0);
});

test("base URL and finite budget profile refuses normalization and embedded secrets", () => {
  for (const baseUrl of ["https://user:secret@slack.com/", "https://slack.com/?token=secret", "https://slack.com/#x",
    "HTTPS://SLACK.COM/", "https://slack.com", "file:///tmp/slack", "https://slack.com:443/"]) {
    assert.throws(() => new SlackOperations({ baseUrl, fetch }), TypeError);
  }
  for (const options of [{ timeoutMs: 0 }, { timeoutMs: Infinity }, { timeoutMs: true },
    { timeoutMs: 2_147_483_648 }, { maxResponseBytes: 0 }, { maxResponseBytes: 1.5 }, { maxResponseBytes: true }]) {
    assert.throws(() => new SlackOperations({ baseUrl: "https://slack.com/", fetch }, options), TypeError);
  }
});

test("structured provider errors fail under HTTP200; only known error codes escape", async () => {
  const values = [["invalid_auth", "authentication", "invalid_auth"], ["missing_scope", "permission", "missing_scope"],
    ["channel_not_found", "not_found", "channel_not_found"], ["ratelimited", "rate_limited", "ratelimited"],
    ["sensitive-private-value", "provider", null], ["__proto__", "provider", null], ["constructor", "provider", null]];
  let index = 0;
  await fixture((_request, response) => reply(response, { ok: false, error: values[index++][0], secret: "private" }), async (server) => {
    const operations = new SlackOperations(selected(server));
    for (const [, kind, code] of values) {
      await assert.rejects(operations.historyPage("C1"), (error) => {
        failure(kind, 200, code)(error);
        assert.ok(!JSON.stringify(error).includes("private"));
        return true;
      });
    }
    assert.equal(server.requests.length, values.length);
  });
});

test("HTTP errors and rate limits do not become successful empty pages or retry", async () => {
  const statuses = [[401, "authentication"], [403, "permission"], [404, "not_found"], [429, "rate_limited"], [503, "http_status"]];
  let index = 0;
  await fixture((_request, response) => reply(response, { secret: "private" },
    { status: statuses[index++][0], headers: { "Retry-After": "123" } }), async (server) => {
    const operations = new SlackOperations(selected(server));
    for (const [status, kind] of statuses) await assert.rejects(operations.historyPage("C1"), failure(kind, status, null, 123));
    assert.equal(server.requests.length, statuses.length);
  });
});

test("redirect is not followed and selected credentials do not reach another listener", async () => {
  await fixture((_request, response) => reply(response, EMPTY), async (other) => {
    await fixture((_request, response) => reply(response, {}, { status: 307, headers: { Location: other.url + "leak" } }), async (server) => {
      await assert.rejects(new SlackOperations(selected(server)).historyPage("C1"), failure("redirect", 307));
      assert.equal(server.requests.length, 1);
      assert.equal(other.requests.length, 0);
    });
  });
});

test("unexpected retargeted host response is refused", async () => {
  await fixture((_request, response) => reply(response, EMPTY), async (server) => {
    const operations = new SlackOperations({ baseUrl: server.url, fetch: () => fetch(server.url + "different") });
    await assert.rejects(operations.historyPage("C1"), failure("redirect", 200));
    assert.equal(server.requests.length, 1, "adapter cannot prevent an untrusted host closure's own side effect");
  });
});

test("strict response profile rejects malformed, duplicate, rounded and incorrectly typed data", async () => {
  const raw = ['{"ok":true,"ok":true,"messages":[]}', '{"ok":true,"messages":[{"ts":1}]}',
    '{"ok":true,"messages":[{"ts":"1\\n"}]}', '{"ok":true,"messages":[],"has_more":1}',
    '{"ok":true,"messages":[],"response_metadata":null}', '{"ok":true,"messages":[],"response_metadata":{"next_cursor":1}}',
    '{"ok":true,"messages":[],"n":9007199254740993}', '{"ok":true,"messages":[],"n":1e999}',
    '{"ok":true,"messages":[],}', '{"ok":true,"messages":[] /* comment */}',
    '{"ok":false}', '{"ok":false,"error":null}', '{"ok":"true","messages":[]}', '{"ok":true}', '[]',
    Buffer.from([0xff]), Buffer.from([0xef, 0xbb, 0xbf, ...Buffer.from(JSON.stringify(EMPTY))])];
  let index = 0;
  await fixture((_request, response) => reply(response, null, { raw: Buffer.from(raw[index++]) }), async (server) => {
    const operations = new SlackOperations(selected(server));
    for (const _ of raw) await assert.rejects(operations.historyPage("C1"), failure("invalid_response", 200));
    assert.equal(server.requests.length, raw.length);
  });
});

test("oversized advertised or streamed bodies and unsupported encoding are refused", async () => {
  await fixture((_request, response) => reply(response, { ...EMPTY, excess: "x".repeat(100) }), async (server) => {
    await assert.rejects(new SlackOperations(selected(server), { maxResponseBytes: 40 }).historyPage("C1"),
      failure("response_too_large", 200));
  });
  await fixture((_request, response) => {
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.write('{"ok":true,"messages":[],"excess":"'); response.end('x'.repeat(100) + '"}');
  }, async (server) => {
    await assert.rejects(new SlackOperations(selected(server), { maxResponseBytes: 40 }).historyPage("C1"),
      failure("response_too_large", 200));
  });
  await fixture((_request, response) => reply(response, EMPTY, { headers: { "Content-Encoding": "private-encoding" } }), async (server) => {
    await assert.rejects(new SlackOperations(selected(server)).historyPage("C1"), failure("unsupported_encoding", 200));
  });
});

test("aborted call never enters transport; host exceptions and abort reasons are redacted", async () => {
  const abort = new AbortController(); abort.abort("secret-reason");
  let calls = 0;
  const operations = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async () => { calls++; throw new Error("secret-host"); } });
  await assert.rejects(operations.historyPage("C1", { signal: abort.signal }), (error) => {
    assert.ok(error instanceof SlackCancelled); assert.ok(!String(error).includes("secret")); return true;
  });
  assert.equal(calls, 0);
  await assert.rejects(operations.historyPage("C1"), (error) => {
    failure("transport")(error); assert.ok(!String(error).includes("secret")); return true;
  });
  assert.equal(calls, 1);
});

test("deadline covers delayed body and does not return partial data", async () => {
  const release = deferred();
  await fixture(async (_request, response) => {
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.write('{"ok":true,"messages":[');
    await release.promise;
    response.end(']}');
  }, async (server) => {
    server.releases.push(release);
    const operation = new SlackOperations(selected(server), { timeoutMs: 100 }).historyPage("C1");
    await assert.rejects(bounded(operation, "stream timeout"), failure("timeout"));
    assert.equal(server.requests.length, 1);
  });
});

test("caller cancellation is joined through fetch and delayed response cancellation", async () => {
  const entered = deferred(); const releaseFetch = deferred(); const enteredClose = deferred(); const releaseClose = deferred();
  let signal; let settled = false; let closeCalls = 0;
  const abort = new AbortController();
  const operations = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async (request) => {
    signal = request.signal; entered.resolve(); await releaseFetch.promise;
    return new Response(new ReadableStream({ cancel: async () => {
      closeCalls++; enteredClose.resolve(); await releaseClose.promise;
    } }), { headers: { "Content-Type": "application/json" } });
  } });
  const result = operations.historyPage("C1", { signal: abort.signal }).then(
    () => ({ unexpected: true }), (error) => ({ error })).finally(() => { settled = true; });
  try {
    await bounded(entered.promise, "fetch entry"); abort.abort("secret");
    await delay(20); assert.equal(signal.aborted, true); assert.equal(settled, false);
    releaseFetch.resolve(); await bounded(enteredClose.promise, "response cancellation");
    await delay(20); assert.equal(settled, false);
    releaseClose.resolve(); const outcome = await bounded(result, "joined operation");
    assert.ok(outcome.error instanceof SlackCancelled); assert.equal(closeCalls, 1);
  } finally { releaseFetch.resolve(); releaseClose.resolve(); await bounded(result, "owner join"); }
});

test("error-path response cancellation remains joined even after deadline", async () => {
  const entered = deferred(); const release = deferred(); let settled = false;
  const operations = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async () =>
    new Response(new ReadableStream({ cancel: async () => { entered.resolve(); await release.promise; } }), { status: 503 })
  }, { timeoutMs: 50 });
  const result = operations.historyPage("C1").then(() => ({ unexpected: true }), (error) => ({ error }))
    .finally(() => { settled = true; });
  try {
    await bounded(entered.promise, "close entry"); await delay(80); assert.equal(settled, false);
    release.resolve(); failure("timeout")((await bounded(result, "close joined")).error);
  } finally { release.resolve(); await bounded(result, "owner join"); }
});
