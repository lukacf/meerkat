// Real loopback HTTP and retained host cleanup. No provider accounts or native authority.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import { GoogleCalendarOperations, CalendarError, CalendarCancelled } from "@rkat/toolkit/connectors/google-calendar";

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
function context(server, account = "first", prefix = "", calendarId = "primary") {
  return { baseUrl: server.url + prefix, calendarId, fetch: (request) => {
    assert.equal(request.redirect, "manual");
    assert.equal(request.credentials, "omit");
    assert.equal(request.cache, "no-store");
    assert.equal(new URL(request.url).host, new URL(server.url).host);
    const headers = new Headers(request.headers);
    headers.set("Authorization", `Bearer fixture-${account}`);
    return fetch(new Request(request, { headers }));
  } };
}
function errorIs(kind, outcome = "unknown", status = null) {
  return (error) => {
    assert.ok(error instanceof CalendarError);
    assert.equal(error.kind, kind);
    assert.equal(error.writeOutcome, outcome);
    assert.equal(error.statusCode, status);
    assert.equal(error.cause, undefined);
    return true;
  };
}

const EVENT = { id: "event0001", etag: '"v1"', updated: "2026-10-01T10:00:00Z", status: "confirmed",
  summary: "kök", start: { date: "2026-10-02" }, end: { date: "2026-10-03" },
  attendees: [], reminders: { useDefault: false }, location: null, future: { preserved: [false, "", null] } };
const collection = (items = [EVENT], extra = {}) => ({ kind: "calendar#events", items, ...extra });
const held = (promise) => promise.then((value) => ({ value }), (error) => ({ error }));

test("public root and subpath share Calendar exports", async () => {
  const root = await import("@rkat/toolkit");
  assert.equal(root.GoogleCalendarOperations, GoogleCalendarOperations);
  assert.equal(root.CalendarError, CalendarError);
  assert.equal(root.CalendarCancelled, CalendarCancelled);
  if (process.env.CALENDAR_EXPECTED_MODULE) {
    assert.equal(import.meta.resolve("@rkat/toolkit/connectors/google-calendar"), process.env.CALENDAR_EXPECTED_MODULE);
  }
});

test("one opaque page at a time preserves an empty middle page and absent terminal items", async () => {
  const cursor = "  opaque+/=%2F końc  ";
  const pages = [collection([EVENT, { id: "gone", status: "cancelled" }, EVENT], { nextPageToken: cursor }),
    collection([], { nextPageToken: cursor + "2" }), { kind: "calendar#events", nextSyncToken: "sync+/=", future: false }];
  let index = 0;
  await fixture((_request, response) => reply(response, pages[index++]), async (server) => {
    const operations = new GoogleCalendarOperations(context(server, "selected", "proxy/", "house+cal@example.com"));
    assert.deepEqual(await operations.listEventsPage({ timeMin: "2026-10-01T00:00:00+02:00",
      timeMax: "2026-10-08T00:00:00+02:00", q: "family", maxResults: 2500,
      singleEvents: true, orderBy: "startTime", showDeleted: false }), pages[0]);
    assert.equal(server.requests.length, 1);
    const first = new URL(server.requests[0].target, server.url);
    assert.equal(first.pathname, "/proxy/calendar/v3/calendars/house%2Bcal%40example.com/events");
    assert.equal(first.searchParams.get("timeMin"), "2026-10-01T00:00:00+02:00");
    assert.equal(first.searchParams.get("maxResults"), "2500");
    assert.equal(first.searchParams.get("showDeleted"), "false");
    assert.deepEqual(await operations.listEventsPage({ pageToken: cursor }), pages[1]);
    assert.equal(new URL(server.requests[1].target, server.url).searchParams.get("pageToken"), cursor);
    assert.deepEqual(await operations.listEventsPage({ pageToken: cursor + "2" }), pages[2]);
    assert.equal(server.requests.length, 3);
    assert.ok(server.requests.every((r) => r.method === "GET" && r.body === ""));
  });
});

test("omitted list options stay omitted and a later page error stays an error", async () => {
  await fixture((request, response) => request.target.includes("pageToken")
    ? reply(response, { private: "provider-secret" }, { status: 503 })
    : reply(response, collection([], { nextPageToken: "next" })), async (server) => {
    const operations = new GoogleCalendarOperations(context(server));
    await operations.listEventsPage();
    assert.equal(server.requests[0].target, "/calendar/v3/calendars/primary/events");
    await assert.rejects(operations.listEventsPage({ pageToken: "next" }), errorIs("http_status", "not_applicable", 503));
    assert.equal(server.requests.length, 2);
  });
});

test("get returns exact event fields and minimal cancelled tombstones", async () => {
  const tombstone = { id: "gone", status: "cancelled", recurringEventId: "series", originalStartTime: { date: "2026-10-03" } };
  await fixture((request, response) => reply(response, request.target.endsWith("/gone") ? tombstone : EVENT), async (server) => {
    const operations = new GoogleCalendarOperations(context(server));
    assert.deepEqual(await operations.getEvent(EVENT.id), EVENT);
    assert.deepEqual(await operations.getEvent("gone"), tombstone);
  });
});

test("calendar and event selectors are encoded as exact single path segments", async () => {
  const id = "event/part?x#%25 ..";
  await fixture((_request, response) => reply(response, { ...EVENT, id }), async (server) => {
    const operations = new GoogleCalendarOperations(context(server, "selected", "prefix/", "house/cal ?#%"));
    assert.equal((await operations.getEvent(id)).id, id);
    assert.equal(server.requests[0].target,
      "/prefix/calendar/v3/calendars/house%2Fcal%20%3F%23%25/events/event%2Fpart%3Fx%23%2525%20..");
  });
});

test("insert snapshot and conditional patch/delete preserve omission null empty false and ETag", async () => {
  const entered = deferred(); const release = deferred();
  const body = { id: EVENT.id, summary: "", location: null, attendees: [], reminders: { useDefault: false },
    start: { date: "2026-10-02" }, end: { date: "2026-10-03" } };
  const original = structuredClone(body);
  await fixture((request, response) => request.method === "DELETE"
    ? (response.writeHead(204, { Connection: "close" }), response.end()) : reply(response, EVENT), async (server) => {
    server.releases.push(release);
    const selected = context(server);
    const operations = new GoogleCalendarOperations({ ...selected, fetch: async (request) => {
      if (request.method === "POST") { entered.resolve(); await release.promise; }
      return selected.fetch(request);
    } });
    const pending = held(operations.insertEvent(body));
    try {
      await bounded(entered.promise, "insert entered");
      body.id = "replaced"; body.attendees.push({ email: "changed@example.com" }); body.reminders.useDefault = true;
      release.resolve();
      assert.deepEqual((await bounded(pending, "insert joined")).value, EVENT);
    } finally { release.resolve(); await bounded(pending, "insert cleanup"); }
    assert.deepEqual(JSON.parse(server.requests[0].body), original);
    assert.equal(server.requests[0].headers["if-match"], undefined);
    assert.deepEqual(await operations.patchEvent(EVENT.id, { location: null, attendees: [], summary: "", guestsCanModify: false },
      { expectedEtag: '"v1"' }), EVENT);
    assert.equal(server.requests[1].headers["if-match"], '"v1"');
    assert.deepEqual(JSON.parse(server.requests[1].body), { location: null, attendees: [], summary: "", guestsCanModify: false });
    assert.equal(await operations.deleteEvent(EVENT.id, { expectedEtag: '"v1"' }), undefined);
    assert.equal(server.requests[2].headers["if-match"], '"v1"');
    assert.equal(server.requests[2].body, "");
    assert.deepEqual(server.requests.map((r) => r.method), ["POST", "PATCH", "DELETE"]);
    assert.ok(server.requests.every((r) => !r.target.includes("?")));
  });
});

test("separate host account closures stay captured at one endpoint and calendar", async () => {
  await fixture((request, response) => reply(response, { ...EVENT, selected: request.headers.authorization }), async (server) => {
    const a = context(server, "a"); const b = context(server, "b");
    const first = new GoogleCalendarOperations(a); const second = new GoogleCalendarOperations(b);
    a.fetch = () => assert.fail("replacement fetch entered"); a.baseUrl = "http://invalid.invalid/"; a.calendarId = "other";
    const results = await Promise.all([first.getEvent(EVENT.id), second.getEvent(EVENT.id)]);
    assert.deepEqual(results.map((r) => r.selected), ["Bearer fixture-a", "Bearer fixture-b"]);
    assert.equal(new Set(server.requests.map((r) => r.target)).size, 1);
  });
});

test("runtime unsupported options and invalid input refuse before any transport", async () => {
  let attempts = 0;
  const selected = { baseUrl: "http://127.0.0.1:1/", calendarId: "primary", fetch: async () => { attempts++; assert.fail("transport entered"); } };
  const operations = new GoogleCalendarOperations(selected);
  for (const baseUrl of ["https://example.com", "https://u:p@example.com/", "http://example.com/?q", "http://example.com/#f", "file:///tmp/", "https://example.com/a/../"]) {
    assert.throws(() => new GoogleCalendarOperations({ ...selected, baseUrl }), TypeError);
  }
  for (const calendarId of ["", ".", "..", null, 1, "x\n", "\ud800"]) assert.throws(() => new GoogleCalendarOperations({ ...selected, calendarId }), TypeError);
  for (const value of [null, true, 0, -1, Infinity, NaN, 2_147_483_648]) {
    assert.throws(() => new GoogleCalendarOperations(selected, { timeoutMs: value }), TypeError);
  }
  for (const value of [null, true, 0, -1, Infinity, 1.5]) {
    assert.throws(() => new GoogleCalendarOperations(selected, { maxResponseBytes: value }), TypeError);
    assert.throws(() => new GoogleCalendarOperations(selected, { maxRequestBytes: value }), TypeError);
  }
  for (const options of [{ fields: "items(id)" }, { syncToken: "s" }, { timeMinimum: "x" }, { q: null }, { pageToken: "" },
    { pageToken: "\ud800" }, { q: "a".repeat(8193) }, { maxResults: true }, { maxResults: 2501 }, { maxResults: 0 },
    { singleEvents: 1 }, { showDeleted: null }, { orderBy: "startTime" }, { orderBy: "other" }, { signal: {} }]) {
    await assert.rejects(operations.listEventsPage(options), TypeError);
  }
  for (const expectedEtag of [null, "", "*", "v1", 'W/"v1"', '"a", "b"', '"a\nb"', '"é"', '"a b"']) {
    await assert.rejects(operations.patchEvent(EVENT.id, {}, { expectedEtag }), TypeError);
    await assert.rejects(operations.deleteEvent(EVENT.id, { expectedEtag }), TypeError);
  }
  await assert.rejects(operations.patchEvent(EVENT.id, { id: "different" }), TypeError);
  for (const invoke of [() => operations.getEvent(EVENT.id, { fields: "id" }),
    () => operations.insertEvent({}, { expectedEtag: '"a"' }), () => operations.deleteEvent(EVENT.id, { sendUpdates: "none" })]) {
    await assert.rejects(invoke(), TypeError);
  }
  let getter = 0;
  await assert.rejects(operations.listEventsPage({ get pageToken() { getter++; return "secret"; } }), TypeError);
  for (const body of [{ value: undefined }, { value: Infinity }, { value: -0 }, { value: 9007199254740992 },
    { value: [, 1] }, { get private() { getter++; return "secret"; } }, { toJSON() { getter++; return {}; } }, []]) {
    await assert.rejects(operations.insertEvent(body), TypeError);
  }
  const cycle = {}; cycle.self = cycle;
  await assert.rejects(operations.insertEvent(cycle), TypeError);
  await assert.rejects(new GoogleCalendarOperations(selected, { maxRequestBytes: 5 }).insertEvent({ text: "long" }), TypeError);
  assert.equal(attempts, 0); assert.equal(getter, 0);
});

test("wrong identities and incomplete write receipts remain unknown", async (t) => {
  for (const [label, body] of [["wrong id", { ...EVENT, id: "other" }], ["missing etag", { id: EVENT.id, updated: EVENT.updated }],
    ["missing updated", { id: EVENT.id, etag: EVENT.etag }], ["write tombstone", { id: EVENT.id, status: "cancelled" }],
    ["wildcard etag", { ...EVENT, etag: "*" }]]) {
    await t.test(label, async () => fixture((_request, response) => reply(response, body), async (server) => {
      const operations = new GoogleCalendarOperations(context(server));
      await assert.rejects(operations.insertEvent({ id: EVENT.id }), errorIs("invalid_response", "unknown", 200));
      await assert.rejects(operations.patchEvent(EVENT.id, {}), errorIs("invalid_response", "unknown", 200));
      if (label === "wrong id") await assert.rejects(operations.getEvent(EVENT.id), errorIs("invalid_response", "not_applicable", 200));
    }));
  }
});

test("malformed JSON and ambiguous page shapes never return a partial page", async (t) => {
  const cases = ["{", '{"kind":"calendar#events","items":[],"items":[]}',
    '{"kind":"calendar#events","x":1e999}', '{"kind":"calendar#events","x":NaN}',
    '{"kind":"calendar#events","x":9007199254740993}', '{"kind":"calendar#events",}',
    '{"kind":"calendar#events",/*comment*/"items":[]}',
    Buffer.from([0xff]), JSON.stringify(collection(null)), JSON.stringify({ items: [] }),
    JSON.stringify(collection([], { nextPageToken: "" })), JSON.stringify(collection([], { nextPageToken: null })),
    JSON.stringify(collection([{ id: EVENT.id, etag: "unquoted", updated: EVENT.updated }])),
    JSON.stringify(collection([EVENT, { ...EVENT, id: "" }]))];
  for (let i = 0; i < cases.length; i++) await t.test(String(i), async () =>
    fixture((_request, response) => reply(response, null, { raw: cases[i] }), async (server) => {
      await assert.rejects(new GoogleCalendarOperations(context(server)).listEventsPage(), errorIs("invalid_response", "not_applicable", 200));
    }));
});

test("write HTTP errors are redacted unknown outcomes without replay", async (t) => {
  for (const [status, kind] of [[401, "authentication"], [403, "permission"], [404, "not_found"], [409, "conflict"],
    [412, "precondition"], [429, "rate_limited"], [500, "http_status"]]) {
    await t.test(String(status), async () => {
      let effects = 0;
      await fixture((_request, response) => { effects++; reply(response, { secret: "provider-private-canary" },
        { status, headers: { "Retry-After": "12" } }); }, async (server) => {
        try { await new GoogleCalendarOperations(context(server)).patchEvent(EVENT.id, {}, { expectedEtag: '"v1"' }); assert.fail("success"); }
        catch (error) {
          errorIs(kind, "unknown", status)(error); assert.equal(error.retryAfterSeconds, 12);
          assert.ok(!JSON.stringify(error).includes("provider-private-canary"));
        }
        assert.equal(effects, 1); assert.equal(server.requests.length, 1);
      });
    });
  }
});

test("200 event receipts and empty 204 deletion are not interchangeable", async () => {
  await fixture((request, response) => request.method === "DELETE" ? reply(response, EVENT)
    : (response.writeHead(204, { Connection: "close" }), response.end()), async (server) => {
    const operations = new GoogleCalendarOperations(context(server));
    await assert.rejects(operations.insertEvent({}), errorIs("http_status", "unknown", 204));
    await assert.rejects(operations.patchEvent(EVENT.id, {}), errorIs("http_status", "unknown", 204));
    await assert.rejects(operations.deleteEvent(EVENT.id), errorIs("http_status", "unknown", 200));
  });
  await fixture((_request, response) => { response.writeHead(204, { "Content-Length": "1", Connection: "close" }); response.end(); }, async (server) => {
    await assert.rejects(new GoogleCalendarOperations(context(server)).deleteEvent(EVENT.id), errorIs("invalid_response", "unknown", 204));
  });
});

test("redirect is refused and never reaches a different route", async () => {
  await fixture((_request, response) => { response.writeHead(307, { Location: "/different", Connection: "close" }); response.end(); }, async (server) => {
    await assert.rejects(new GoogleCalendarOperations(context(server)).insertEvent({}), errorIs("redirect", "unknown", 307));
    assert.equal(server.requests.length, 1);
  });
});

test("streamed response bounds and media encoding fail without partial success", async (t) => {
  for (const [label, headers, raw, expected] of [
    ["size", {}, JSON.stringify(collection()), "response_too_large"],
    ["encoding", { "Content-Encoding": "gzip" }, "x", "unsupported_encoding"],
    ["charset", { "Content-Type": "application/json; charset=iso-8859-1" }, "{}", "invalid_response"],
    ["media", { "Content-Type": "text/plain" }, "{}", "invalid_response"]]) {
    await t.test(label, async () => fixture((_request, response) => {
      response.writeHead(200, { "Content-Type": "application/json", Connection: "close", ...headers });
      response.write(raw.slice(0, 4)); response.end(raw.slice(4));
    }, async (server) => {
      await assert.rejects(new GoogleCalendarOperations(context(server), { maxResponseBytes: 64 }).listEventsPage(),
        errorIs(expected, "not_applicable", 200));
    }));
  }
});

test("one deadline covers a progressing streamed body", async () => {
  const entered = deferred(); const peerClosed = deferred();
  await fixture(async (_request, response) => {
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.write('{"kind":"calendar#events","items":['); entered.resolve();
    const interval = setInterval(() => { if (!response.destroyed) response.write(" "); }, 5);
    response.once("close", () => peerClosed.resolve());
    try { await bounded(peerClosed.promise, "deadline socket closed"); } finally { clearInterval(interval); }
  }, async (server) => {
    const pending = held(new GoogleCalendarOperations(context(server), { timeoutMs: 70 }).listEventsPage());
    await bounded(entered.promise, "stream entered");
    errorIs("timeout", "not_applicable")((await bounded(pending, "deadline joined")).error);
    await bounded(peerClosed.promise, "peer closes before teardown");
  });
});

test("later-page cancellation joins entered response cleanup before rejection", async () => {
  const cancelEntered = deferred(); const releaseCleanup = deferred(); const readEntered = deferred();
  const peerClosed = deferred(); let cleanupFinished = false; let settled = false;
  await fixture((request, response) => {
    if (!request.target.includes("pageToken")) return reply(response, collection([], { nextPageToken: "later" }));
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.write('{"kind":"calendar#events","items":[');
    response.once("close", () => peerClosed.resolve());
  }, async (server) => {
    server.releases.push(releaseCleanup);
    const selected = context(server); const controller = new AbortController();
    const operations = new GoogleCalendarOperations({ ...selected, fetch: async (request) => {
      const response = await selected.fetch(request);
      if (!request.url.includes("pageToken")) return response;
      const inner = response.body.getReader();
      const body = new ReadableStream({
        async pull(output) { readEntered.resolve(); try { const item = await inner.read();
          if (item.done) output.close(); else output.enqueue(item.value); } catch (error) { output.error(error); } },
        async cancel() { cancelEntered.resolve();
          try { await inner.cancel(); } finally { await releaseCleanup.promise; inner.releaseLock(); cleanupFinished = true; }
        },
      });
      return new Response(body, { status: response.status, headers: response.headers });
    } });
    const first = await operations.listEventsPage();
    const pending = held(operations.listEventsPage({ pageToken: first.nextPageToken, signal: controller.signal })).then((value) => { settled = true; return value; });
    try {
      await bounded(readEntered.promise, "later response read entered");
      controller.abort("private-abort-canary");
      await bounded(cancelEntered.promise, "local response cancel entered");
      await bounded(peerClosed.promise, "real socket closed while local cleanup held");
      await delay(30);
      assert.equal(settled, false); assert.equal(cleanupFinished, false);
      releaseCleanup.resolve();
      const result = await bounded(pending, "cancelled page joined");
      assert.equal(cleanupFinished, true); assert.ok(result.error instanceof CalendarCancelled);
      errorIs("cancelled", "not_applicable")(result.error); assert.equal(server.requests.length, 2);
    } finally { releaseCleanup.resolve(); await bounded(pending, "retained page operation"); }
  });
});

test("post-entry write cancellation is unknown and does not undo the remote effect", async () => {
  const entered = deferred(); const release = deferred(); const finished = deferred(); let effects = 0;
  await fixture(async (_request, response) => {
    entered.resolve(); await release.promise; effects++; reply(response, EVENT); finished.resolve();
  }, async (server) => {
    server.releases.push(release);
    const controller = new AbortController();
    const pending = held(new GoogleCalendarOperations(context(server)).insertEvent({ id: EVENT.id }, { signal: controller.signal }));
    try {
      await bounded(entered.promise, "remote write entered"); controller.abort("private-abort");
      const result = await bounded(pending, "local write cancellation");
      assert.ok(result.error instanceof CalendarCancelled); errorIs("cancelled")(result.error);
      assert.equal(effects, 0); release.resolve(); await bounded(finished.promise, "remote may complete");
      assert.equal(effects, 1); assert.equal(server.requests.length, 1);
    } finally { release.resolve(); await bounded(pending, "write retained"); }
  });
});

test("deadline and cancellation join a host Fetch that ignores abort", async (t) => {
  for (const reason of ["timeout", "cancelled"]) await t.test(reason, async () => {
    const entered = deferred(); const release = deferred(); let settled = false; let bodyCancelled = false;
    await fixture((_request, response) => reply(response, EVENT), async (server) => {
      server.releases.push(release);
      const selected = context(server); const controller = new AbortController();
      const operations = new GoogleCalendarOperations({ ...selected, fetch: async (request) => {
        // Deliberately ignore signal but retain this real transport until its response is returned.
        const copy = new Request(request, { signal: new AbortController().signal });
        const response = await selected.fetch(copy); entered.resolve(); await release.promise;
        return new Response(new ReadableStream({ async cancel() { await response.body.cancel(); bodyCancelled = true; } }),
          { status: response.status, headers: response.headers });
      } }, { timeoutMs: reason === "timeout" ? 60 : 2000 });
      const pending = held(operations.insertEvent({}, { signal: controller.signal })).then((value) => { settled = true; return value; });
      try {
        await bounded(entered.promise, "host entered actual HTTP");
        if (reason === "cancelled") controller.abort("private");
        await delay(90); assert.equal(settled, false);
        release.resolve(); const result = await bounded(pending, "host response cleanup joined");
        errorIs(reason)(result.error); assert.equal(bodyCancelled, true); assert.equal(server.requests.length, 1);
      } finally { release.resolve(); await bounded(pending, "host retained"); }
    });
  });
});

test("already aborted sends nothing and arbitrary host errors remain redacted", async () => {
  let attempts = 0;
  const operations = new GoogleCalendarOperations({ baseUrl: "http://127.0.0.1:1/", calendarId: "primary",
    fetch: async () => { attempts++; throw new Error("private-host-canary"); } });
  const controller = new AbortController(); controller.abort("private-reason");
  await assert.rejects(operations.deleteEvent(EVENT.id, { signal: controller.signal }), errorIs("cancelled"));
  assert.equal(attempts, 0);
  await assert.rejects(operations.insertEvent({}), errorIs("transport"));
  assert.equal(attempts, 1);
});
