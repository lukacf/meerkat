// Real loopback HTTP fixtures. No Google account, credentials or delivery claims.
import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import net from "node:net";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import {
  GmailOperations, GmailError, GmailCancelled,
} from "@rkat/toolkit/connectors/gmail";

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
function context(server, account = "first", prefix = "", userId = "me") {
  // The host selects the account. These are local fixture values, not real credentials.
  return { baseUrl: server.url + prefix, userId, fetch: (request) => {
    assert.equal(request.redirect, "manual");
    assert.equal(request.credentials, "omit");
    const headers = new Headers(request.headers);
    headers.set("Authorization", `Bearer fixture-${account}`);
    return fetch(new Request(request, { headers }));
  } };
}
function errorIs(kind, outcome = "unknown", status = null) {
  return (error) => {
    assert.ok(error instanceof GmailError);
    assert.equal(error.kind, kind);
    assert.equal(error.writeOutcome, outcome);
    assert.equal(error.statusCode, status);
    assert.equal(error.cause, undefined);
    return true;
  };
}

const MIME = new TextEncoder().encode("From: sender@example.com\r\nTo: recipient@example.com\r\n"
  + "Cc: copy@example.com\r\nBcc: hidden@example.com\r\nMessage-ID: <unchanged@example.com>\r\n"
  + "Subject: Summary\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nExact summary.\r\n");
const RECEIPT = { id: "sent_01", threadId: "thread_01", labelIds: ["SENT"], future: { keep: [false, null] } };
function msg(id = "message_01") { return { id, threadId: "thread_01" }; }

test("public root and subpath exports select the same connector", async () => {
  const root = await import("@rkat/toolkit");
  assert.equal(root.GmailOperations, GmailOperations);
  assert.equal(root.GmailError, GmailError);
  if (process.env.GMAIL_EXPECTED_MODULE) {
    assert.equal(import.meta.resolve("@rkat/toolkit/connectors/gmail"), process.env.GMAIL_EXPECTED_MODULE);
  }
});

test("one page preserves absent fields, opaque token, order, duplicates and estimates", async () => {
  const pages = [{ messages: [msg("b"), msg("a"), msg("b")], nextPageToken: "opaque+/= token",
    resultSizeEstimate: 999, future: { keep: [false, null, "kök"] } },
  { messages: [msg("b")], nextPageToken: "opaque+/= token", resultSizeEstimate: 1 }, {}];
  await fixture((_request, response) => reply(response, pages.shift()), async (server) => {
    const operations = new GmailOperations(context(server, "selected", "proxy/", "a+b@example.com"));
    const first = structuredClone(pages[0]);
    assert.deepEqual(await operations.listMessagesPage({ query: "from:sender@example.com after:123",
      maxResults: 2, includeSpamTrash: true, labelIds: ["INBOX", "Label_2"] }), first);
    assert.equal(server.requests.length, 1);
    const url = new URL(server.requests[0].target, server.url);
    assert.equal(url.pathname, "/proxy/gmail/v1/users/a%2Bb%40example.com/messages");
    assert.equal(url.searchParams.get("q"), "from:sender@example.com after:123");
    assert.deepEqual(url.searchParams.getAll("labelIds"), ["INBOX", "Label_2"]);
    assert.equal(url.searchParams.get("includeSpamTrash"), "true");
    assert.equal(url.searchParams.get("maxResults"), "2");
    const second = structuredClone(pages[0]);
    assert.deepEqual(await operations.listMessagesPage({ pageToken: first.nextPageToken }), second);
    assert.equal(new URL(server.requests[1].target, server.url).searchParams.get("pageToken"), first.nextPageToken);
    assert.deepEqual(await operations.listMessagesPage(), {});
    assert.equal(server.requests.length, 3, "no auto-page loop on repeated cursor");
    assert.ok(server.requests.every((r) => r.method === "GET" && r.body === ""));
  });
});

test("later page error is exposed, not returned as successful complete retrieval", async () => {
  await fixture((request, response) => request.target.includes("pageToken")
    ? reply(response, { error: "private" }, { status: 503 })
    : reply(response, { messages: [msg()], nextPageToken: "next" }), async (server) => {
    const operations = new GmailOperations(context(server));
    const first = await operations.listMessagesPage();
    await assert.rejects(operations.listMessagesPage({ pageToken: first.nextPageToken }), errorIs("http_status", "not_applicable", 503));
    assert.equal(server.requests.length, 2);
  });
});

test("get preserves selected representation, repeated headers and nested MIME data", async () => {
  const nested = { ...msg(), internalDate: "123", snippet: "private text", payload: {
    mimeType: "multipart/mixed", headers: [{ name: "X-Test", value: "a" }, { name: "X-Test", value: "b" }],
    parts: [{ partId: "0", mimeType: "text/plain", body: { size: 3, data: "YWJj" } },
      { partId: "1", filename: "file.bin", body: { size: 1024, attachmentId: "attachment" }, future: null }],
  }, future: { keep: [true, null] } };
  await fixture((request, response) => {
    const format = new URL(request.target, "http://fixture/").searchParams.get("format");
    reply(response, format === "raw" ? { ...msg(), raw: "AP_+", future: null } : nested);
  }, async (server) => {
    const operations = new GmailOperations(context(server));
    for (const format of ["full", "metadata", "minimal", "raw"]) {
      const options = { format, ...(format === "metadata" ? { metadataHeaders: ["Subject", "X-Test", "X-Test"] } : {}) };
      assert.deepEqual(await operations.getMessage("message_01", options), format === "raw" ? { ...msg(), raw: "AP_+", future: null } : nested);
    }
    const params = new URL(server.requests[1].target, server.url).searchParams;
    assert.deepEqual(params.getAll("metadataHeaders"), ["Subject", "X-Test", "X-Test"]);
    assert.equal(server.requests.length, 4, "no attachment fetching");
  });
});

test("two accounts at one endpoint remain separately selected after context mutation", async () => {
  await fixture((request, response) => reply(response, { ...msg(), receipt: request.headers.authorization }), async (server) => {
    const a = context(server, "a", "", "a@example.com");
    const b = context(server, "b", "", "b@example.com");
    const operationsA = new GmailOperations(a); const operationsB = new GmailOperations(b);
    a.baseUrl = "http://invalid.invalid/"; a.userId = "b@example.com";
    a.fetch = () => assert.fail("replacement transport entered");
    const receipts = await Promise.all([operationsA.getMessage("message_01"), operationsB.getMessage("message_01")]);
    assert.deepEqual(receipts.map((r) => r.receipt), ["Bearer fixture-a", "Bearer fixture-b"]);
    assert.deepEqual(new Set(server.requests.map((r) => r.target)), new Set([
      "/gmail/v1/users/a%40example.com/messages/message_01?format=full",
      "/gmail/v1/users/b%40example.com/messages/message_01?format=full",
    ]));
  });
});

test("send copies the exact Uint8Array view before suspension and preserves every MIME byte", async () => {
  const release = deferred(); const entered = deferred();
  await fixture((request, response) => {
    assert.deepEqual(Object.keys(JSON.parse(request.body)), ["raw"]);
    assert.deepEqual(Buffer.from(JSON.parse(request.body).raw, "base64url"), Buffer.from(MIME));
    assert.equal(request.target, "/gmail/v1/users/me/messages/send");
    assert.equal(request.method, "POST");
    reply(response, RECEIPT);
  }, async (server) => {
    server.releases.push(release);
    const selected = context(server);
    const operations = new GmailOperations({ ...selected, fetch: async (request) => {
      entered.resolve(); await release.promise; return selected.fetch(request);
    } });
    const backing = new Uint8Array(MIME.length + 8); backing.set(MIME, 4);
    const view = backing.subarray(4, 4 + MIME.length);
    const result = operations.sendRaw(view); const settled = result.then((value) => ({ value }), (error) => ({ error }));
    try {
      await bounded(entered.promise, "host transport entered");
      backing.fill(0); release.resolve();
      assert.deepEqual((await bounded(settled, "exact MIME send joined")).value, RECEIPT);
      assert.equal(server.requests.length, 1);
    } finally { release.resolve(); await bounded(settled, "send retained"); }
  });
});

test("binary MIME bytes, including zero and high bytes, are not decoded or repaired", async () => {
  const bytes = Uint8Array.from([0, 1, 127, 128, 254, 255]);
  await fixture((request, response) => {
    assert.deepEqual(Buffer.from(JSON.parse(request.body).raw, "base64url"), Buffer.from(bytes));
    assert.match(JSON.parse(request.body).raw, /^[A-Za-z0-9_=-]+$/);
    reply(response, RECEIPT);
  }, async (server) => { assert.deepEqual(await new GmailOperations(context(server)).sendRaw(bytes), RECEIPT); });
});

test("invalid local selection and input cause zero host attempts", async () => {
  await fixture(() => assert.fail("invalid input sent HTTP"), async (server) => {
    let attempts = 0; const selected = context(server);
    const operations = new GmailOperations({ ...selected, fetch: (r) => { attempts++; return selected.fetch(r); } });
    for (const userId of ["", "../me", "x/y@example.com", "me\n", "a@b\n", "é@example.com", 1, null]) {
      assert.throws(() => new GmailOperations({ ...selected, userId }), TypeError);
    }
    for (const baseUrl of [server.url + "?x", server.url + "#x", server.url + "a/../", "https://user:secret@example.com/", "file:///tmp/"]) {
      assert.throws(() => new GmailOperations({ ...selected, baseUrl }), TypeError);
    }
    for (const timeoutMs of [0, -1, NaN, Infinity, null, true, 2_147_483_648]) {
      assert.throws(() => new GmailOperations(selected, { timeoutMs }), TypeError);
    }
    for (const limit of [0, -1, null, true, 1.1, Infinity]) {
      for (const key of ["maxResponseBytes", "maxMessageBytes"]) assert.throws(() => new GmailOperations(selected, { [key]: limit }), TypeError);
    }
    for (const maxResults of [0, 501, 1.5, null, true]) await assert.rejects(operations.listMessagesPage({ maxResults }), TypeError);
    for (const query of ["", "\ud800", "x\n", "é".repeat(4097), null]) await assert.rejects(operations.listMessagesPage({ query }), TypeError);
    for (const pageToken of ["", "\udc00", "x\r", null]) await assert.rejects(operations.listMessagesPage({ pageToken }), TypeError);
    for (const labelIds of [null, "INBOX", Array(1), Array(101).fill("INBOX"), ["x/y"]]) await assert.rejects(operations.listMessagesPage({ labelIds }), TypeError);
    await assert.rejects(operations.listMessagesPage({ includeSpamTrash: 1 }), TypeError);
    for (const id of ["", "../x", "a?b", "a\n"]) await assert.rejects(operations.getMessage(id), TypeError);
    for (const options of [{ format: "unknown" }, { format: null }, { metadataHeaders: ["Subject"] },
      { format: "metadata", metadataHeaders: ["a:b"] }, { format: "metadata", metadataHeaders: Array(1) }]) {
      await assert.rejects(operations.getMessage("message_01", options), TypeError);
    }
    const detached = new Uint8Array([1]); structuredClone(detached, { transfer: [detached.buffer] });
    for (const bytes of ["mime", [], new Uint8Array(), new Uint8Array(new SharedArrayBuffer(8)), detached]) {
      await assert.rejects(operations.sendRaw(bytes), TypeError);
    }
    await assert.rejects(new GmailOperations(selected, { maxMessageBytes: 1 }).sendRaw(MIME), TypeError);
    for (const signal of [null, {}, "cancel"]) await assert.rejects(operations.sendRaw(MIME, { signal }), TypeError);
    assert.equal(attempts, 0); assert.equal(server.requests.length, 0);
  });
});

test("wrong message identity and malformed page shapes refuse unchanged provider data", async () => {
  for (const value of [msg("other"), { ...msg(), payload: [] }, { ...msg(), raw: false }, { ...msg(), labelIds: [null] }, false]) {
    await fixture((_request, response) => reply(response, value), async (server) => {
      await assert.rejects(new GmailOperations(context(server)).getMessage("message_01"), errorIs("invalid_response", "not_applicable"));
    });
  }
  for (const value of [[], null, { messages: [false] }, { messages: [{ id: "a" }] }, { nextPageToken: "" },
    { nextPageToken: null }, { resultSizeEstimate: -1 }, { resultSizeEstimate: true }, { resultSizeEstimate: 0.1 }]) {
    await fixture((_request, response) => reply(response, value), async (server) => {
      await assert.rejects(new GmailOperations(context(server)).listMessagesPage(), errorIs("invalid_response", "not_applicable"));
    });
  }
  await fixture((_request, response) => reply(response, msg()), async (server) => {
    await assert.rejects(new GmailOperations(context(server)).getMessage("message_01", { format: "raw" }), errorIs("invalid_response", "not_applicable"));
  });
});

test("redirect refusal does not expose credentials to the destination or retry", async () => {
  await fixture(() => assert.fail("redirect destination entered"), async (target) => {
    await fixture((_request, response) => reply(response, null, { status: 307, headers: { Location: target.url } }), async (source) => {
      await assert.rejects(new GmailOperations(context(source)).sendRaw(MIME), errorIs("redirect", "unknown", 307));
      assert.equal(source.requests.length, 1); assert.equal(target.requests.length, 0);
    });
  });
});

test("HTTP refusals remain unknown with fixed safe metadata and no hidden retry", async () => {
  for (const [status, kind] of [[400, "http_status"], [401, "authentication"], [403, "permission"],
    [404, "not_found"], [429, "rate_limited"], [503, "http_status"]]) {
    await fixture((_request, response) => reply(response, { secret: "private-provider-canary" },
      { status, headers: { "Retry-After": "17" } }), async (server) => {
      await assert.rejects(new GmailOperations(context(server)).sendRaw(MIME), (error) => {
        errorIs(kind, "unknown", status)(error);
        assert.equal(error.retryAfterSeconds, 17);
        assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes("private-provider-canary"));
        assert.ok(!JSON.stringify(error).includes(server.url));
        return true;
      });
      assert.equal(server.requests.length, 1);
    });
  }
});

test("read HTTP error and host exception are safe and preserve outcome distinction", async () => {
  await fixture((_request, response) => reply(response, null, { status: 404 }), async (server) => {
    await assert.rejects(new GmailOperations(context(server)).getMessage("message_01"), errorIs("not_found", "not_applicable", 404));
    const operations = new GmailOperations({ ...context(server), fetch: () => { throw new Error("host-secret-canary"); } });
    await assert.rejects(operations.sendRaw(MIME), (error) => {
      errorIs("transport")(error); assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes("host-secret-canary")); return true;
    });
    assert.equal(server.requests.length, 1);
  });
});

test("strict JSON, invalid UTF-8 and malformed success are uncertain send receipts", async () => {
  for (const raw of ['{"id":"a","id":"b","threadId":"t"}', '{"id":"a","\\u0069d":"b","threadId":"t"}',
    '{"id":"a","threadId":"t","x":1e999}', '{"id":"a","threadId":"t","x":9007199254740993}',
    '{"id":"a","threadId":"t",}', '/*comment*/{}', '{} trailing', '[', "[".repeat(129) + "]".repeat(129),
    Buffer.from([0xff]), Buffer.from([0xef, 0xbb, 0xbf, 123, 125])]) {
    await fixture((_request, response) => reply(response, null, { raw: Buffer.from(raw) }), async (server) => {
      await assert.rejects(new GmailOperations(context(server)).sendRaw(MIME), errorIs("invalid_response", "unknown", 200));
      assert.equal(server.requests.length, 1);
    });
  }
  for (const value of [null, false, [], {}, { id: "a" }, { id: "", threadId: "t" }]) {
    await fixture((_request, response) => reply(response, value), async (server) => {
      await assert.rejects(new GmailOperations(context(server)).sendRaw(MIME), errorIs("invalid_response"));
      assert.equal(server.requests.length, 1);
    });
  }
});

test("identity encoding, JSON type and declared or streamed byte ceilings are enforced", async () => {
  for (const chunked of [false, true]) {
    await fixture((_request, response) => {
      const body = JSON.stringify({ ...RECEIPT, padding: "x".repeat(200) });
      if (!chunked) reply(response, null, { raw: Buffer.from(body) });
      else { response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
        response.write(body.slice(0, 32)); response.end(body.slice(32)); }
    }, async (server) => {
      await assert.rejects(new GmailOperations(context(server), { maxResponseBytes: 64 }).sendRaw(MIME),
        errorIs("response_too_large", "unknown", 200));
      assert.equal(server.requests.length, 1);
    });
  }
  for (const [headers, kind] of [[{ "Content-Encoding": "gzip" }, "unsupported_encoding"], [{ "Content-Type": "text/html" }, "invalid_response"]]) {
    await fixture((_request, response) => reply(response, RECEIPT, { headers }), async (server) => {
      await assert.rejects(new GmailOperations(context(server)).sendRaw(MIME), errorIs(kind, "unknown", 200));
    });
  }
});

test("post-entry cancellation is unknown while provider can still complete", async () => {
  const entered = deferred(); const release = deferred(); const effect = deferred(); const disconnected = deferred();
  await fixture(async (_request, response) => {
    response.once("close", () => disconnected.resolve()); entered.resolve();
    await release.promise; effect.resolve("provider continued");
    if (!response.destroyed) reply(response, RECEIPT);
  }, async (server) => {
    server.releases.push(release);
    const controller = new AbortController();
    const pending = new GmailOperations(context(server)).sendRaw(MIME, { signal: controller.signal });
    const settled = pending.then((value) => ({ value }), (error) => ({ error }));
    try {
      await bounded(entered.promise, "provider entry"); controller.abort("private-abort-reason");
      const result = await bounded(settled, "cancelled send settled");
      assert.ok(result.error instanceof GmailCancelled); errorIs("cancelled")(result.error);
      assert.ok(!`${result.error.stack} ${JSON.stringify(result.error)}`.includes("private-abort-reason"));
      await bounded(disconnected.promise, "fetch socket closed");
      release.resolve(); assert.equal(await bounded(effect.promise, "late provider completion"), "provider continued");
      assert.equal(server.requests.length, 1);
    } finally { controller.abort(); release.resolve(); await bounded(settled, "operation joined"); }
  });
});

test("one total deadline survives response progress and retires the actual connection", { timeout: 10_000 }, async () => {
  const entered = deferred(); const closed = deferred();
  await fixture(async (_request, response) => {
    response.once("close", () => closed.resolve());
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.write('{"id":"a","threadId":"t","x":"'); entered.resolve();
    for (let i = 0; i < 10 && !response.destroyed; i++) {
      await delay(200); if (!response.destroyed) response.write("x");
    }
    if (!response.destroyed) response.end('"}');
  }, async (server) => {
    const pending = new GmailOperations(context(server), { timeoutMs: 1_000 }).sendRaw(MIME);
    const settled = pending.then((value) => ({ value }), (error) => ({ error }));
    await bounded(entered.promise, "HTTP entry before deadline");
    errorIs("timeout")((await bounded(settled, "total deadline")).error);
    await bounded(closed.promise, "timed-out socket closed"); assert.equal(server.requests.length, 1);
  });
});

test("post-acceptance connection loss preserves exact one-attempt MIME and unknown effect", async () => {
  let providerReceipt;
  await fixture((request, response) => { providerReceipt = Buffer.from(JSON.parse(request.body).raw, "base64url"); response.destroy(); }, async (server) => {
    await assert.rejects(new GmailOperations(context(server)).sendRaw(MIME), errorIs("transport"));
    assert.deepEqual(providerReceipt, Buffer.from(MIME)); assert.equal(server.requests.length, 1);
  });
});

test("already-aborted signal sends nothing and read cancellation has no write outcome", async () => {
  const entered = deferred(); const release = deferred();
  await fixture(async (_request, response) => { entered.resolve(); await release.promise; if (!response.destroyed) reply(response, msg()); }, async (server) => {
    server.releases.push(release); const controller = new AbortController(); controller.abort("private");
    const operations = new GmailOperations(context(server));
    await assert.rejects(operations.sendRaw(MIME, { signal: controller.signal }), errorIs("cancelled"));
    assert.equal(server.requests.length, 0);
    const read = new AbortController(); const settled = operations.getMessage("message_01", { signal: read.signal }).then((value) => ({ value }), (error) => ({ error }));
    try {
      await bounded(entered.promise, "read entry"); read.abort();
      errorIs("cancelled", "not_applicable")((await bounded(settled, "read cancel")).error);
    } finally { read.abort(); release.resolve(); await bounded(settled, "read joined"); }
  });
});

test("an abort-ignoring host Fetch is retained until its same promise settles", async () => {
  for (const kind of ["cancelled", "timeout"]) {
    const entered = deferred(); const release = deferred(); const abort = deferred(); const cancelledBody = deferred();
    await fixture((_request, response) => { entered.resolve(); reply(response, RECEIPT); }, async (server) => {
      server.releases.push(release); const selected = context(server); let settled = false; let attempts = 0;
      const controller = new AbortController();
      const operations = new GmailOperations({ ...selected, fetch: async (request) => {
        attempts++; request.signal.addEventListener("abort", () => abort.resolve(), { once: true });
        // Deliberately nonconforming host retains the actual fetch response, then exposes its body later.
        const response = await selected.fetch(new Request(request, { signal: new AbortController().signal }));
        await release.promise;
        return new Response(new ReadableStream({ cancel() { cancelledBody.resolve(); return response.body.cancel(); } }),
          { status: 200, headers: { "Content-Type": "application/json" } });
      } }, { timeoutMs: 1_000 });
      const result = operations.sendRaw(MIME, { signal: controller.signal }).then((value) => ({ value }), (error) => ({ error }));
      const tracked = result.then((value) => { settled = true; return value; });
      try {
        await bounded(entered.promise, "real host HTTP entry");
        if (kind === "cancelled") controller.abort();
        await bounded(abort.promise, "operation abort observed by host");
        await Promise.resolve(); assert.equal(settled, false, "abort is not joined transport settlement");
        release.resolve(); errorIs(kind)((await bounded(tracked, "same Fetch joined")).error);
        await bounded(cancelledBody.promise, "late response body cancelled");
        assert.equal(attempts, 1); assert.equal(server.requests.length, 1);
      } finally { controller.abort(); release.resolve(); await bounded(tracked, "host promise joined"); }
    });
  }
});

test("stalled borrowed Gmail readers cancel once and join cleanup before settlement", async (t) => {
  for (const status of [200, 403]) {
    for (const write of [false, true]) {
      for (const kind of ["cancelled", "timeout"]) {
        for (const rejectCleanup of [false, true]) {
          await t.test(`${status} ${write ? "send" : "read"} ${kind} cleanup ${rejectCleanup ? "rejects" : "resolves"}`, async () => {
            const readEntered = deferred(); const cancelEntered = deferred(); const releaseCleanup = deferred();
            const abort = new AbortController(); const requests = [];
            let streamController; let readerCancels = 0; let sourceCancels = 0;
            let cleanupFinished = false; let settled = false;
            const secret = "private-stalled-body-cleanup-detail";
            const stream = new ReadableStream({
              start(controller) { streamController = controller; },
              // This native stream is cancellable but does not react to the request signal.
              pull() { readEntered.resolve(); },
              async cancel() {
                sourceCancels++; cancelEntered.resolve();
                await releaseCleanup.promise;
                cleanupFinished = true;
                if (rejectCleanup) throw new Error(secret);
              },
            }, { highWaterMark: 0 });
            const getReader = stream.getReader.bind(stream);
            stream.getReader = (...args) => {
              const reader = getReader(...args); const cancel = reader.cancel.bind(reader);
              reader.cancel = (...values) => { readerCancels++; return cancel(...values); };
              return reader;
            };
            const operations = new GmailOperations({
              baseUrl: "https://gmail.googleapis.com/", userId: "me",
              fetch: async request => {
                requests.push(request);
                return new Response(stream, { status, headers: { "Content-Type": "application/json" } });
              },
            }, { timeoutMs: kind === "timeout" ? 30 : 5_000 });
            const operation = write ? operations.sendRaw(MIME, { signal: abort.signal })
              : operations.listMessagesPage({ signal: abort.signal });
            const tracked = operation.then(value => ({ value }), error => ({ error }))
              .then(result => { settled = true; return result; });
            try {
              await bounded(readEntered.promise, "borrowed body read entered");
              if (kind === "cancelled") abort.abort(secret);
              await bounded(cancelEntered.promise, "owned reader cancellation entered");
              assert.equal(requests[0].signal.aborted, true);
              assert.equal(readerCancels, 1); assert.equal(sourceCancels, 1);
              assert.equal(settled, false); assert.equal(cleanupFinished, false);
              // A later caller signal cannot replace a deadline or start a second cleanup.
              abort.abort(secret);
              releaseCleanup.resolve();
              const result = await bounded(tracked, "same borrowed cleanup joined");
              errorIs(kind, write ? "unknown" : "not_applicable")(result.error);
              assert.equal(result.error instanceof GmailCancelled, kind === "cancelled");
              assert.equal(result.error.providerReason, null);
              assert.ok(!`${result.error.stack} ${JSON.stringify(result.error)}`.includes(secret));
              assert.equal(cleanupFinished, true); assert.equal(stream.locked, false);
              assert.equal(readerCancels, 1); assert.equal(sourceCancels, 1);
              assert.equal(requests.length, 1);
              assert.equal(requests[0].method, write ? "POST" : "GET");
            } finally {
              abort.abort(); releaseCleanup.resolve();
              // Retire the fixture even when a regression fails to call cancel.
              try { streamController.close(); } catch {}
              await bounded(tracked, "stalled fixture operation joined");
            }
          });
        }
      }
    }
  }
});

test("completed borrowed Gmail bodies release their reader and detach caller cancellation", async () => {
  const abort = new AbortController(); let request; let readerCancels = 0;
  const stream = new ReadableStream({
    start(controller) { controller.enqueue(new TextEncoder().encode(JSON.stringify(RECEIPT))); controller.close(); },
  });
  const getReader = stream.getReader.bind(stream);
  stream.getReader = (...args) => {
    const reader = getReader(...args); const cancel = reader.cancel.bind(reader);
    reader.cancel = (...values) => { readerCancels++; return cancel(...values); };
    return reader;
  };
  const operations = new GmailOperations({
    baseUrl: "https://gmail.googleapis.com/", userId: "me",
    fetch: async value => { request = value; return new Response(stream, { headers: { "Content-Type": "application/json" } }); },
  });
  assert.deepEqual(await operations.sendRaw(MIME, { signal: abort.signal }), RECEIPT);
  assert.equal(stream.locked, false); assert.equal(readerCancels, 0);
  abort.abort();
  assert.equal(request.signal.aborted, false); assert.equal(readerCancels, 0);
});

test("reader cancellation is awaited and cleanup failures remain safe", async () => {
  const entered = deferred(); const release = deferred();
  await fixture((_request, response) => reply(response, RECEIPT), async (server) => {
    server.releases.push(release); const selected = context(server); let settled = false;
    const operations = new GmailOperations({ ...selected, fetch: async (request) => {
      const response = await selected.fetch(request); await response.body.cancel();
      return new Response(new ReadableStream({
        start(c) { c.enqueue(new Uint8Array(200)); },
        async cancel() { entered.resolve(); await release.promise; throw new Error("private-cleanup-canary"); },
      }), { headers: { "Content-Type": "application/json" } });
    } }, { maxResponseBytes: 64 });
    const result = operations.sendRaw(MIME).then((value) => ({ value }), (error) => ({ error }));
    const tracked = result.then((value) => { settled = true; return value; });
    try {
      await bounded(entered.promise, "entered actual reader cancel");
      assert.equal(settled, false); release.resolve();
      const error = (await bounded(tracked, "reader cleanup joined")).error;
      errorIs("response_too_large", "unknown", 200)(error);
      assert.ok(!`${error.stack} ${JSON.stringify(error)}`.includes("private-cleanup-canary"));
    } finally { release.resolve(); await bounded(tracked, "owned cleanup joined"); }
  });
});

test("fixture retains accepted pre-HTTP sockets and joins after body assertion failure", async () => {
  let owner;
  await assert.rejects(fixture(() => assert.fail("no HTTP expected"), async (server) => {
    owner = server;
    const socket = net.connect({ host: "127.0.0.1", port: server.server.address().port });
    socket.on("error", () => {}); const closed = new Promise((resolve) => socket.once("close", resolve));
    try { await bounded(server.accepted.promise, "real accepted socket"); }
    finally { socket.destroy(); await bounded(closed, "caller socket retired"); }
    throw new Error("intentional body failure");
  }), /intentional body failure/);
  assert.equal(owner.joined, true); assert.equal(owner.sockets.size, 0); assert.ok(owner.socketClosures.length > 0);
});
