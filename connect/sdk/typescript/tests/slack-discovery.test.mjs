import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { setTimeout as delay } from "node:timers/promises";
import { SlackOperations, SlackError, SlackCancelled } from "@rkat/toolkit/connectors/slack";

function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
async function bounded(promise) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("synthetic fixture did not join")), 5_000);
  })]); } finally { clearTimeout(timer); }
}
const values = {
  auth: { ok: true, team_id: "T1", user_id: "U1", bot_id: null, extra: { allowed: false } },
  list: { ok: true, channels: [{ id: "C1", future: null }, { id: "C1", future: false }], extra: [null, false] },
  info: { ok: true, channel: { id: "C1", extra: { shared: false, subject: null } } },
};
const methods = {
  auth: (slack, signal) => slack.authTest(signal === undefined ? {} : { signal }),
  list: (slack, signal) => slack.listConversationsPage(signal === undefined ? {} : { signal }),
  info: (slack, signal) => slack.getConversation("C1", signal === undefined ? {} : { signal }),
};
function json(value, init = {}) {
  return new Response(JSON.stringify(value), { ...init, headers: { "Content-Type": "application/json", ...init.headers } });
}
function safe(kind, status = null) {
  return (error) => {
    assert.ok(error instanceof SlackError);
    assert.equal(error.kind, kind);
    assert.equal(error.statusCode, status);
    assert.equal(error.cause, undefined);
    assert.ok(!String(error).includes("synthetic-private"));
    assert.ok(!JSON.stringify(error).includes("synthetic-private"));
    return true;
  };
}
async function fixture(run) {
  const calls = []; const sockets = new Set(); const tasks = []; const closedSockets = [];
  const server = http.createServer((request, response) => {
    const task = (async () => {
      const chunks = [];
      for await (const chunk of request) chunks.push(chunk);
      calls.push({ url: request.url, method: request.method, headers: request.headers, body: Buffer.concat(chunks).toString() });
      const method = new URL(request.url, "http://fixture/").pathname.split("/").at(-1);
      const value = method === "auth.test" ? { ...values.auth, selected: request.headers.authorization }
        : method === "conversations.list" ? { ...values.list, response_metadata: { next_cursor: "opaque /+= kök" } }
        : values.info;
      const bytes = Buffer.from(JSON.stringify(value));
      response.writeHead(200, { "Content-Type": "application/json", "Content-Length": String(bytes.length), Connection: "close" });
      response.end(bytes);
    })();
    tasks.push(task.then(() => null, (error) => error));
  });
  server.on("connection", (socket) => {
    sockets.add(socket);
    closedSockets.push(new Promise((resolve) => socket.once("close", () => { sockets.delete(socket); resolve(); })));
    socket.on("error", () => {});
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const baseUrl = `http://127.0.0.1:${server.address().port}/proxy/`;
  const selected = (account) => ({ baseUrl, fetch: (request) => {
    assert.equal(request.redirect, "manual"); assert.equal(request.credentials, "omit"); assert.equal(request.cache, "no-store");
    const headers = new Headers(request.headers); headers.set("Authorization", `Bearer synthetic-${account}`);
    return fetch(new Request(request, { headers }));
  } });
  try { await run({ calls, selected, baseUrl }); }
  finally {
    const closed = new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
    for (const socket of sockets) socket.destroy();
    await bounded(closed); await bounded(Promise.all(closedSockets));
    const errors = (await bounded(Promise.all(tasks))).filter((error) => error !== null);
    assert.deepEqual(errors, []); assert.equal(sockets.size, 0);
  }
}

test("discovery uses actual loopback HTTP with exact manual pages and selected host credentials", async () => {
  await fixture(async ({ calls, selected, baseUrl }) => {
    const context = selected("first"); const slack = new SlackOperations(context);
    context.baseUrl = "https://wrong.example.test/"; context.fetch = () => assert.fail("changed context used");
    assert.equal((await slack.authTest()).selected, "Bearer synthetic-first");
    const page = await slack.listConversationsPage({ types: ["im", "private_channel"], excludeArchived: true, limit: 7, teamId: "T2" });
    assert.deepEqual(page.channels, values.list.channels);
    assert.equal(calls.length, 2);
    await slack.listConversationsPage({ types: ["im", "private_channel"], excludeArchived: true, limit: 7, teamId: "T2", cursor: page.response_metadata.next_cursor });
    assert.deepEqual(await slack.getConversation("C1", { includeLocale: true, includeNumMembers: true }), values.info);
    assert.equal(calls.length, 4);
    assert.equal(calls[0].method, "POST"); assert.equal(calls[0].body, "");
    assert.match(calls[0].headers["content-type"], /^application\/x-www-form-urlencoded/);
    assert.equal(calls[0].url, "/proxy/api/auth.test");
    assert.deepEqual(Object.fromEntries(new URL(calls[2].url, baseUrl).searchParams), {
      types: "im,private_channel", exclude_archived: "true", limit: "7", team_id: "T2", cursor: "opaque /+= kök",
    });
    assert.deepEqual(Object.fromEntries(new URL(calls[3].url, baseUrl).searchParams), {
      channel: "C1", include_locale: "true", include_num_members: "true",
    });
    assert.ok(calls.every((call) => call.headers.authorization === "Bearer synthetic-first"));
    assert.ok(calls.slice(1).every((call) => call.method === "GET" && call.body === ""));
  });
});

test("two selected discovery contexts remain isolated and observations are never cached", async () => {
  await fixture(async ({ calls, selected }) => {
    const first = new SlackOperations(selected("first")); const second = new SlackOperations(selected("second"));
    const observations = await Promise.all([first.authTest(), second.authTest(), first.authTest()]);
    assert.deepEqual(observations.map((value) => value.selected), ["Bearer synthetic-first", "Bearer synthetic-second", "Bearer synthetic-first"]);
    assert.equal(calls.length, 3);
  });
});

test("list captures mutable types, flags, cursor and team before the borrowed Fetch suspends", async () => {
  const release = deferred(); let request;
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async (value) => {
    request = value; await release.promise; return json(values.list);
  } });
  const options = { types: ["private_channel", "mpim"], excludeArchived: true, cursor: "first+=", limit: 2, teamId: "T1" };
  const result = slack.listConversationsPage(options);
  options.types.splice(0, 2, "im"); options.excludeArchived = false; options.cursor = "different"; options.teamId = "T2"; options.limit = 8;
  release.resolve(); await result;
  assert.deepEqual(Object.fromEntries(new URL(request.url).searchParams), {
    types: "private_channel,mpim", exclude_archived: "true", cursor: "first+=", limit: "2", team_id: "T1",
  });
});

test("list preserves absent empty null and nonterminal cursor metadata without inferring completion", async () => {
  const pages = [{ ok: true, channels: [] }, { ok: true, channels: [], response_metadata: {} },
    { ok: true, channels: [], response_metadata: { next_cursor: "" } },
    { ok: true, channels: [], response_metadata: { next_cursor: null } },
    { ok: true, channels: [], response_metadata: { next_cursor: "more", unknown: false }, has_more: null }];
  let calls = 0;
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => json(pages[calls++]) });
  for (const page of pages) assert.deepEqual(await slack.listConversationsPage(), page);
  assert.equal(calls, pages.length);
});

test("discovery local argument profile rejects before transport", async () => {
  let calls = 0;
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => { calls++; return json(values.list); } });
  for (const options of [{ types: [] }, { types: new Array(1) }, { types: ["im", , "mpim"] },
    { types: "im" }, { types: ["im", "im"] }, { types: ["unknown"] },
    { types: null }, { types: ["im", "mpim", "private_channel", "public_channel", "im"] }, { types: [null] },
    { excludeArchived: 1 }, { excludeArchived: null }, { limit: true }, { limit: 0 }, { limit: 1000 }, { limit: 1.5 },
    { teamId: "../T1" }, { teamId: null }, { teamId: "T".repeat(129) }, { cursor: "" }, { cursor: "a\n" },
    { cursor: "\ud800" }, { cursor: "é".repeat(4097) }, { signal: {} }]) {
    await assert.rejects(slack.listConversationsPage(options), TypeError);
  }
  for (const channel of ["", "../C1", "C1\n", null, true, "C".repeat(129)]) await assert.rejects(slack.getConversation(channel), TypeError);
  for (const options of [{ includeLocale: 1 }, { includeNumMembers: null }, { signal: {} }]) await assert.rejects(slack.getConversation("C1", options), TypeError);
  await assert.rejects(slack.authTest({ signal: {} }), TypeError);
  assert.equal(calls, 0);
});

for (const [profile, invalid] of Object.entries({
  auth: [{ ok: true, team_id: null, user_id: "U1" }, { ok: true, team_id: "T1" }, { ok: true, team_id: "T1", user_id: "U1\n" }],
  list: [{ ok: true, channels: null }, { ok: true, channels: [{ id: null }] }, { ok: true, channels: [], response_metadata: null },
    { ok: true, channels: [], response_metadata: { next_cursor: 3 } }],
  info: [{ ok: true, channel: null }, { ok: true, channel: { id: "C2" } }, { ok: true, channel: { id: "C1\n" } }],
})) {
  test(profile + " refuses malformed supported success without partial data", async () => {
    let calls = 0;
    const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => json(invalid[calls++]) });
    for (const _ of invalid) await assert.rejects(methods[profile](slack), safe("invalid_response", 200));
    assert.equal(calls, invalid.length);
  });
}

for (const profile of Object.keys(methods)) {
  test(profile + " keeps full unknown JSON and rejects strict JSON and complete-body framing failures", async () => {
    const payload = JSON.stringify(values[profile]);
    const invalid = [payload.replace('{"ok":true', '{"ok":true,"ok":true'), payload.slice(0, -1) + ',"huge":9007199254740993}',
      payload.slice(0, -1) + ',"n":1e999}', "[]"];
    let calls = 0;
    const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => {
      const raw = invalid[calls++];
      return new Response(raw, { headers: { "Content-Type": "application/json" } });
    } });
    for (const _ of invalid) await assert.rejects(methods[profile](slack), safe("invalid_response", 200));
    for (const length of ["x", String(Buffer.byteLength(payload) - 1), String(Buffer.byteLength(payload) + 1)]) {
      const framed = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => json(values[profile], { headers: { "Content-Length": length } }) });
      await assert.rejects(methods[profile](framed), safe("invalid_response", 200));
    }
    const exact = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => json(values[profile]) });
    assert.deepEqual(await methods[profile](exact), values[profile]);
  });

  test(profile + " pre-abort and host exception never expose caller diagnostics", async () => {
    let calls = 0; const abort = new AbortController(); abort.abort("synthetic-private-reason");
    const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => { calls++; throw new Error("synthetic-private-host"); } });
    await assert.rejects(methods[profile](slack, abort.signal), (error) => { safe("cancelled")(error); assert.ok(error instanceof SlackCancelled); return true; });
    assert.equal(calls, 0);
    await assert.rejects(methods[profile](slack), safe("transport")); assert.equal(calls, 1);
  });

  test(profile + " joins borrowed Fetch and response cancellation before settling abort", async () => {
    const entered = deferred(); const releaseFetch = deferred(); const closeEntered = deferred(); const releaseClose = deferred();
    const abort = new AbortController(); let settled = false; let calls = 0; let closes = 0; let request;
    const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async (value) => {
      calls++; request = value; entered.resolve(); await releaseFetch.promise;
      return new Response(new ReadableStream({ cancel: async () => { closes++; closeEntered.resolve(); await releaseClose.promise; } }),
        { headers: { "Content-Type": "application/json" } });
    } });
    const outcome = methods[profile](slack, abort.signal).then(() => ({ bad: true }), (error) => ({ error })).finally(() => { settled = true; });
    try {
      await bounded(entered.promise); abort.abort("synthetic-private"); await delay(10);
      assert.equal(request.signal.aborted, true); assert.equal(settled, false);
      releaseFetch.resolve(); await bounded(closeEntered.promise); await delay(10); assert.equal(settled, false);
      releaseClose.resolve(); const result = await bounded(outcome);
      assert.ok(result.error instanceof SlackCancelled); safe("cancelled")(result.error);
      assert.equal(calls, 1); assert.equal(closes, 1);
    } finally { releaseFetch.resolve(); releaseClose.resolve(); await bounded(outcome); }
  });

  test(profile + " deadline still joins error cleanup and never retries", async () => {
    const closeEntered = deferred(); const releaseClose = deferred(); let settled = false; let calls = 0;
    const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => {
      calls++; return new Response(new ReadableStream({ cancel: async () => { closeEntered.resolve(); await releaseClose.promise; } }), { status: 503 });
    } }, { timeoutMs: 40 });
    const outcome = methods[profile](slack).then(() => ({ bad: true }), (error) => ({ error })).finally(() => { settled = true; });
    try {
      await bounded(closeEntered.promise); await delay(70); assert.equal(settled, false);
      releaseClose.resolve(); safe("timeout")((await bounded(outcome)).error); assert.equal(calls, 1);
    } finally { releaseClose.resolve(); await bounded(outcome); }
  });

  test(profile + " provider refusal redirect encoding and byte bounds remain closed safe outcomes", async () => {
    const rows = [
      [() => json({ ok: false, error: "synthetic-private", details: "synthetic-private" }), "provider", 200],
      [() => json({ ok: false, error: "invalid_auth" }), "authentication", 200],
      [() => json({}, { status: 429, headers: { "Retry-After": "12" } }), "rate_limited", 429],
      [() => json({}, { status: 307, headers: { Location: "https://wrong.example.test/" } }), "redirect", 307],
      [() => json(values[profile], { headers: { "Content-Encoding": "gzip" } }), "unsupported_encoding", 200],
      [() => json(values[profile], { headers: { "Content-Length": "999999999" } }), "response_too_large", 200],
    ];
    let calls = 0;
    const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => rows[calls++][0]() });
    for (const [, kind, status] of rows) await assert.rejects(methods[profile](slack), safe(kind, status));
    assert.equal(calls, rows.length);
  });
}

test("public discovery example is cold and compares explicit host identity without selecting a replacement", async () => {
  const { observeSelectedIdentity } = await import("../examples/slack-discovery.mjs");
  let calls = 0;
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async () => { calls++; return json(values.auth); } });
  assert.equal(calls, 0);
  assert.deepEqual(await observeSelectedIdentity(slack, { teamId: "T1", userId: "U1" }), values.auth);
  await assert.rejects(observeSelectedIdentity(slack, { teamId: "T2", userId: "U1" }), /selected Slack identity/);
  assert.equal(calls, 2);
});
