import test from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { SlackOperations, SlackCancelled } from "@rkat/toolkit/connectors/slack";

const defaults = { start: "10", end: "20", rootOldest: "0", maxRequests: 10, maxMessages: 20 };
const page = (messages = [], metadata = {}) => ({ ok: true, messages, ...metadata });
const kinds = (result) => result.coverage_gaps.map((gap) => gap.kind);
function selected(pages, { maxResponseBytes = 2_097_152, token = "synthetic-one" } = {}) {
  const calls = [];
  const context = { baseUrl: "https://slack.invalid/proxy/", fetch: async (request) => {
    calls.push({ request, token });
    const value = pages[calls.length - 1];
    assert.notEqual(value, undefined, "unexpected extra request");
    return value instanceof Response ? value : new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });
  } };
  return { calls, context, slack: new SlackOperations(context, { maxResponseBytes }) };
}
function deferred() {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
}
async function bounded(promise) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error("synthetic fixture did not join")), 2_000);
  })]); } finally { clearTimeout(timer); }
}

test("public interval capability exists", () => {
  assert.equal(typeof SlackOperations.prototype.readInterval, "function");
});

test("old roots, exact rows and duplicates use fixed bounds through page owners", async () => {
  const first = { ts: "15.000", reply_count: null, unknown: [null, false] };
  const child = { ts: "12.00", thread_ts: "5.0", text: "child", unknown: { n: null } };
  const { slack, calls } = selected([
    page([{ ts: "5.0", text: "older private body", reply_count: 0 }, first, { ts: "20" }],
      { has_more: true, response_metadata: { next_cursor: "opaque +/= ö" } }),
    page([{ ts: "5.0", text: "older private body" }, child, child,
      { ts: "10", thread_ts: "5.0" }, { ts: "20", thread_ts: "5.0" }]),
    page([first]), page(),
  ]);
  const result = await slack.readInterval("C1", { ...defaults, pageLimit: 7, includeAllMetadata: true });
  assert.deepEqual(result.messages.map((row) => row.message), [first, child, child, { ts: "10", thread_ts: "5.0" }, first]);
  assert.equal(JSON.stringify(result).includes("older private body"), false);
  assert.equal(result.termination, "finished"); assert.equal(result.history.state, "exhausted");
  assert.deepEqual(result.threads.map((thread) => thread.root_ts), ["5.0", "15.000"]);
  assert.deepEqual(result.coverage_gaps, []);
  assert.deepEqual(calls.map(({ request }) => new URL(request.url).pathname), [
    "/proxy/api/conversations.history", "/proxy/api/conversations.replies",
    "/proxy/api/conversations.replies", "/proxy/api/conversations.history",
  ]);
  calls.forEach(({ request, token }, index) => {
    const query = new URL(request.url).searchParams;
    assert.equal(query.get("channel"), "C1"); assert.equal(query.get("oldest"), [0, 3].includes(index) ? "0" : "10");
    assert.equal(query.get("latest"), "20"); assert.equal(query.get("inclusive"), "true");
    assert.equal(query.get("limit"), "7"); assert.equal(query.get("include_all_metadata"), "true");
    assert.equal(token, "synthetic-one"); assert.equal(request.redirect, "manual");
    assert.equal(request.credentials, "omit"); assert.equal(request.cache, "no-store");
  });
  assert.equal(new URL(calls[3].request.url).searchParams.get("cursor"), "opaque +/= ö");
  assert.deepEqual(result.limits, { max_requests: 10, max_messages: 20, page_limit: 7, max_response_bytes: 2_097_152, max_result_bytes: 262144 });
});

test("small request budget makes thread progress before history continuation", async () => {
  const child = { ts: "11", thread_ts: "1" };
  const { slack, calls } = selected([
    page([{ ts: "1", reply_count: 0 }], { response_metadata: { next_cursor: "history-next" } }), page([child]),
  ]);
  const result = await slack.readInterval("C1", { ...defaults, maxRequests: 2 });
  assert.deepEqual(calls.map(({ request }) => new URL(request.url).pathname.split("/").at(-1)), ["conversations.history", "conversations.replies"]);
  assert.deepEqual(result.messages, [{ source: "replies", root_ts: "1", message: child }]);
  assert.equal(result.termination, "request_budget"); assert.equal(result.history.state, "pending");
  assert.equal(result.history.next_cursor, "history-next"); assert.equal(result.threads[0].state, "exhausted");
});

test("over-budget projection admits no rows, roots, metadata or progress from refused page", async () => {
  const { slack, calls } = selected([
    page([], { has_more: true, response_metadata: { next_cursor: "accepted" } }),
    page([{ ts: "11" }, { ts: "12" }, { ts: "13" }], { has_more: false, is_limited: true, response_metadata: { next_cursor: "refused" } }),
  ]);
  const result = await slack.readInterval("C1", { ...defaults, maxMessages: 2 });
  assert.equal(calls.length, 2); assert.deepEqual(result.messages, []); assert.deepEqual(result.threads, []);
  assert.deepEqual(result.history, { state: "stopped", pages: 1, stop_reason: "message_budget", next_cursor: "accepted", has_more: true });
  assert.equal(result.termination, "message_budget"); assert.deepEqual(kinds(result), ["message_budget"]);
});

for (const [name, metadata, reason] of [
  ["absent", {}, "missing_cursor"], ["null", { next_cursor: null }, "unusable_cursor"],
  ["empty", { next_cursor: "" }, "unusable_cursor"], ["control", { next_cursor: "bad\n" }, "unusable_cursor"],
]) test(`has_more with ${name} cursor is not exhaustion`, async () => {
  const { slack, calls } = selected([page([], { has_more: true, response_metadata: metadata })]);
  const result = await slack.readInterval("C1", defaults);
  assert.equal(calls.length, 1); assert.equal(result.history.state, "stopped");
  assert.equal(result.history.stop_reason, reason); assert.equal(result.termination, "finished");
  assert.deepEqual(kinds(result), [reason]);
  assert.equal(Object.hasOwn(result.history, "next_cursor"), Object.hasOwn(metadata, "next_cursor"));
  if (Object.hasOwn(metadata, "next_cursor")) assert.equal(result.history.next_cursor, metadata.next_cursor);
});

for (const [name, metadata] of [["absent", {}], ["null", { next_cursor: null }], ["empty", { next_cursor: "" }]]) {
  test(`terminal cursor ${name} remains exact`, async () => {
    const { slack, calls } = selected([page([], { response_metadata: metadata })]);
    const result = await slack.readInterval("C1", defaults);
    assert.equal(calls.length, 1); assert.equal(result.history.state, "exhausted");
    assert.equal(Object.hasOwn(result.history, "next_cursor"), Object.hasOwn(metadata, "next_cursor"));
    assert.equal(Object.hasOwn(result.history, "has_more"), false);
    if (Object.hasOwn(metadata, "next_cursor")) assert.equal(result.history.next_cursor, metadata.next_cursor);
  });
}

test("fresh cursor overrides false hint, cycle stops without a time fallback", async () => {
  const { slack, calls } = selected([
    page([], { has_more: false, response_metadata: { next_cursor: "A" } }),
    page([], { response_metadata: { next_cursor: "B" } }), page([], { response_metadata: { next_cursor: "A" } }),
  ]);
  const result = await slack.readInterval("C1", defaults);
  assert.deepEqual(calls.map(({ request }) => new URL(request.url).searchParams.get("cursor")), [null, "A", "B"]);
  assert.ok(calls.every(({ request }) => new URL(request.url).searchParams.get("oldest") === "0"));
  assert.equal(result.history.stop_reason, "cursor_cycle"); assert.equal(result.history.pages, 3);
  assert.deepEqual(kinds(result), ["pagination_contradiction", "cursor_cycle"]);
});

for (const [name, marker, expected] of [["true", true, "retention_limited"], ["null", null, "invalid_coverage_metadata"],
  ["number", 0, "invalid_coverage_metadata"], ["string", "true", "invalid_coverage_metadata"]]) {
  test(`retention metadata ${name} is observed without snapshot claims`, async () => {
    const { slack } = selected([page([], { is_limited: marker })]);
    const result = await slack.readInterval("C1", { ...defaults, rootOldest: "5" });
    assert.deepEqual(kinds(result), ["older_roots_unsearched", expected]);
    assert.equal(result.termination, "finished"); assert.equal(Object.hasOwn(result, "complete"), false);
  });
}

for (const [name, extra, reason] of [
  ["null", { thread_ts: null }, "invalid_thread_reference"],
  ["future", { thread_ts: "13" }, "invalid_thread_reference"],
  ["false", { thread_ts: false }, "invalid_thread_reference"],
  ["broadcast-missing", { subtype: "thread_broadcast" }, "invalid_broadcast_reference"],
  ["broadcast-null-root", { subtype: "thread_broadcast", thread_ts: "1", root: null }, "invalid_broadcast_reference"],
  ["broadcast-wrong-root", { subtype: "reply_broadcast", thread_ts: "1", root: { ts: "2" } }, "invalid_broadcast_reference"],
]) test(`malformed ${name} linkage retains row without guessing a root`, async () => {
  const row = { ts: "12", ...extra }; const { slack, calls } = selected([page([row])]);
  const result = await slack.readInterval("C1", defaults);
  assert.equal(calls.length, 1); assert.deepEqual(result.threads, []);
  assert.deepEqual(result.messages, [{ source: "history", root_ts: null, message: row }]);
  assert.deepEqual(kinds(result), [reason]);
});

test("broadcast reference crosses explicit discovery floor while preserving nested selected data", async () => {
  const broadcast = { ts: "12", subtype: "thread_broadcast", thread_ts: "1", root: { ts: "1", text: "nested rendering" } };
  const { slack, calls } = selected([page([broadcast]), page([{ ts: "13", thread_ts: "1" }])]);
  const result = await slack.readInterval("C1", { ...defaults, rootOldest: "5" });
  assert.equal(new URL(calls[1].request.url).searchParams.get("ts"), "1");
  assert.deepEqual(result.messages[0].message, broadcast); assert.deepEqual(kinds(result), ["older_roots_unsearched"]);
});

test("discovery range, end root and nonthreadable skips do not spend queries", async () => {
  const { slack, calls } = selected([page([{ ts: "4" }, { ts: "21", thread_ts: "1" }, { ts: "20.00" },
    { ts: "12", subtype: "channel_join" }, { ts: "13", subtype: "channel_leave" }])]);
  const result = await slack.readInterval("C1", { ...defaults, rootOldest: "5" });
  assert.equal(calls.length, 1); assert.deepEqual(result.threads, []); assert.equal(result.skipped_non_threadable, 2);
  assert.deepEqual(result.messages.map((row) => row.message.ts), ["12", "13"]);
  assert.deepEqual(kinds(result), ["older_roots_unsearched", "history_outside_discovery", "history_outside_discovery"]);
});

test("reply chain mismatch is not claimed as observed linkage", async () => {
  const rows = [{ ts: "11", thread_ts: "4" }, { ts: "12" }];
  const { slack } = selected([page([{ ts: "1" }]), page(rows)]);
  const result = await slack.readInterval("C1", defaults);
  assert.deepEqual(result.messages.map((row) => row.message), rows);
  assert.ok(result.messages.every((row) => row.root_ts === "1"));
  assert.deepEqual(kinds(result), ["reply_chain_mismatch", "reply_chain_mismatch"]);
});

test("thread_not_found continues other roots while permission failure stops globally", async () => {
  const { slack, calls } = selected([page([{ ts: "1" }, { ts: "2" }, { ts: "3" }]),
    { ok: false, error: "thread_not_found" }, { ok: false, error: "missing_scope", raw: "synthetic-private" }]);
  const result = await slack.readInterval("C1", defaults);
  assert.equal(calls.length, 3); assert.equal(result.termination, "request_failure");
  assert.deepEqual(result.threads.map((thread) => thread.state), ["stopped", "stopped", "pending"]);
  assert.deepEqual(result.threads[0].error, { kind: "not_found", status_code: 200, provider_code: "thread_not_found", retry_after_seconds: null });
  assert.equal(result.threads[1].error.provider_code, "missing_scope");
  assert.equal(JSON.stringify(result).includes("synthetic-private"), false);
});

test("exact decimal boundaries beyond Number precision and separate selected bindings", async () => {
  const start = "9007199254740993.00000000000000001", end = "9007199254740993.00000000000000003";
  const row = { ts: "9007199254740993.00000000000000002", subtype: "channel_join", false: false };
  for (const token of ["synthetic-A", "synthetic-B"]) {
    const { slack, calls, context } = selected([page([{ ts: start, subtype: "channel_join" }, row, { ts: end }])], { token });
    context.fetch = () => assert.fail("changed context reused"); context.baseUrl = "https://wrong.invalid/";
    const result = await slack.readInterval("C1", { start, end, rootOldest: start, maxRequests: 1, maxMessages: 3 });
    assert.equal(result.messages.length, 2); assert.deepEqual(result.messages[1].message, row);
    assert.equal(new URL(calls[0].request.url).searchParams.get("oldest"), start); assert.equal(calls[0].token, token);
  }
});

for (const [name, changes] of [
  ["start", { start: "20" }], ["end", { end: "10" }], ["floor", { rootOldest: "11" }], ["numeric-ts", { start: 10 }],
  ["bool-budget", { maxRequests: true }], ["zero-budget", { maxRequests: 0 }], ["unsafe-budget", { maxRequests: 9007199254740992 }],
  ["zero-messages", { maxMessages: 0 }], ["fraction-budget", { maxMessages: 1.5 }], ["limit", { pageLimit: 1000 }],
  ["flag", { includeAllMetadata: 1 }], ["signal", { signal: {} }],
]) test(`invalid local ${name} refuses before request`, async () => {
  const { slack, calls } = selected([]);
  await assert.rejects(slack.readInterval("C1", { ...defaults, ...changes }), TypeError);
  assert.deepEqual(calls, []);
});

test("options are captured before the first await", async () => {
  const entered = deferred(), release = deferred(); const calls = [];
  const options = { ...defaults, maxRequests: 2 };
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async (request) => {
    calls.push(request); if (calls.length === 1) { entered.resolve(); await release.promise; }
    return new Response(JSON.stringify(calls.length === 1 ? page([], { response_metadata: { next_cursor: "next" } }) : page()),
      { headers: { "Content-Type": "application/json" } });
  } });
  const pending = slack.readInterval("C1", options);
  await bounded(entered.promise); Object.assign(options, { start: "99", end: "100", rootOldest: "98", maxRequests: 1, maxMessages: 1, pageLimit: 1 });
  release.resolve(); const result = await bounded(pending);
  assert.equal(calls.length, 2); assert.equal(result.start, "10"); assert.equal(result.limits.max_requests, 2);
  assert.equal(new URL(calls[1].url).searchParams.get("oldest"), "0"); assert.equal(new URL(calls[1].url).searchParams.get("limit"), "15");
});

test("malformed page and byte limit yield safe partial failures, not empty exhaustion", async () => {
  for (const [body, kind] of [['{"ok":true,"messages":[],"messages":[]}', "invalid_response"], ["x".repeat(200), "response_too_large"]]) {
    const { slack, calls } = selected([new Response(body, { headers: { "Content-Type": "application/json" } })], { maxResponseBytes: 100 });
    const result = await slack.readInterval("C1", defaults);
    assert.equal(calls.length, 1); assert.deepEqual(result.messages, []); assert.equal(result.history.pages, 0);
    assert.equal(result.history.state, "stopped"); assert.equal(result.history.error.kind, kind); assert.equal(result.termination, "request_failure");
  }
});

test("entry abort refuses before transport and never returns partial success", async () => {
  const abort = new AbortController(); abort.abort("synthetic-private");
  const { slack, calls } = selected([]);
  await assert.rejects(slack.readInterval("C1", { ...defaults, signal: abort.signal }), (error) => {
    assert.ok(error instanceof SlackCancelled); assert.equal(error.cause, undefined);
    assert.equal(String(error).includes("synthetic-private"), false); return true;
  });
  assert.equal(calls.length, 0);
});

test("cancellation remains pending until retained fetch and response cleanup join", async () => {
  const entered = deferred(), releaseFetch = deferred(), closing = deferred(), releaseClose = deferred();
  const abort = new AbortController(); let settled = false, closeCalls = 0, calls = 0;
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async () => {
    calls++; entered.resolve(); await releaseFetch.promise;
    return new Response(new ReadableStream({ cancel: async () => {
      closeCalls++; closing.resolve(); await releaseClose.promise;
    } }), { headers: { "Content-Type": "application/json" } });
  } });
  const pending = slack.readInterval("C1", { ...defaults, signal: abort.signal }).then(
    () => ({ unexpected: true }), (error) => ({ error })).finally(() => { settled = true; });
  try {
    await bounded(entered.promise); abort.abort("synthetic-private");
    await delay(5); assert.equal(settled, false);
    releaseFetch.resolve(); await bounded(closing.promise); await delay(5); assert.equal(settled, false);
    releaseClose.resolve(); const outcome = await bounded(pending);
    assert.ok(outcome.error instanceof SlackCancelled); assert.equal(closeCalls, 1); assert.equal(calls, 1);
  } finally { releaseFetch.resolve(); releaseClose.resolve(); await bounded(pending); }
});
