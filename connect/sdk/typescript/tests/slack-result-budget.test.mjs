import test from "node:test";
import assert from "node:assert/strict";
import { SlackOperations, SlackCancelled, SlackError } from "@rkat/toolkit/connectors/slack";

const defaults = { start: "10", end: "20", rootOldest: "0", maxRequests: 10, maxMessages: 100,
  maxResultBytes: 4096, resumable: true };
const page = (messages = [], extra = {}) => ({ ok: true, messages, ...extra });
const row = (text, extra = {}) => ({ ts: "11", subtype: "channel_join", text, ...extra });
const json = (value) => new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });
const selection = { channel: "C1", start: "10", end: "20", root_oldest: "0", page_limit: 15, include_all_metadata: false };
const checkpoint = (cursor = null, seen = []) => ({ schema_version: 1, selection,
  pending: [{ root_ts: null, cursor, seen_cursors: seen }], discovered_roots: [] });
// Count ASCII JSON independently of the owner's replacement serializer.
function bytes(value) {
  const text = JSON.stringify(value); let size = 0;
  for (let i = 0; i < text.length; i++) size += text.charCodeAt(i) >= 127 ? 6 : 1;
  return size;
}
function fixture(pages) {
  const calls = [];
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async (request) => {
    calls.push(request); const value = pages[calls.length - 1];
    assert.notEqual(value, undefined, "unexpected additional request");
    return value instanceof Response ? value : json(value);
  } });
  return { slack, calls };
}

for (const value of [null, false, "4096", 4095, 16777217, 4096.5, NaN, Infinity]) {
  test(`invalid full-result budget ${String(value)} refuses before transport`, async () => {
    const { slack, calls } = fixture([]);
    await assert.rejects(slack.readInterval("C1", { ...defaults, maxResultBytes: value }), TypeError);
    assert.equal(calls.length, 0);
  });
}

test("omitted budget defaults to 262144 and both inclusive endpoints are accepted", async () => {
  for (const maxResultBytes of [undefined, 4096, 16777216]) {
    const { slack } = fixture([page()]);
    const result = await slack.readInterval("C1", { ...defaults, maxResultBytes });
    assert.equal(result.limits.max_result_bytes, maxResultBytes ?? 262144);
    assert.ok(bytes(result) <= result.limits.max_result_bytes);
  }
});

test("whole terminal result fits exactly and one extra byte refuses the entire page", async () => {
  const empty = fixture([page([row("")])]);
  const base = await empty.slack.readInterval("C1", defaults);
  const text = "x".repeat(4096 - bytes(base));
  const exact = fixture([page([row(text)])]);
  const admitted = await exact.slack.readInterval("C1", defaults);
  assert.equal(bytes(admitted), 4096); assert.equal(admitted.termination, "finished");
  const over = fixture([page([row(text + "x")])]);
  const stopped = await over.slack.readInterval("C1", defaults);
  assert.equal(stopped.termination, "result_budget");
  assert.deepEqual(stopped.messages, []); assert.equal(stopped.skipped_non_threadable, 0);
  assert.equal(stopped.history.pages, 0); assert.equal(stopped.history.stop_reason, "result_budget");
  assert.deepEqual(stopped.coverage_gaps.map((x) => x.kind), ["result_budget"]);
  assert.deepEqual(stopped.continuation, checkpoint()); assert.equal(over.calls.length, 1);
  assert.ok(bytes(stopped) <= 4096);
});

test("ASCII measurement includes Unicode, DEL, controls, quotes, backslashes and finite numbers", async () => {
  const message = row(("é😀\u007f\n\"\\\ud800").repeat(180), { smallest: 5e-324, largest: Number.MAX_SAFE_INTEGER, fractional: 1.2345e-100 });
  const limited = fixture([page([message])]);
  const stopped = await limited.slack.readInterval("C1", defaults);
  assert.equal(stopped.termination, "result_budget"); assert.deepEqual(stopped.messages, []);
  const wider = fixture([page([message])]);
  const result = await wider.slack.readInterval("C1", { ...defaults, maxResultBytes: 16384, continuation: stopped.continuation });
  assert.equal(result.termination, "finished"); assert.deepEqual(result.messages[0].message, message);
  assert.ok(bytes(result) > Buffer.byteLength(JSON.stringify(result)));
  assert.ok(bytes(result) <= result.limits.max_result_bytes);
});

test("overflow after a prior page commits no new rows, roots, gaps, metadata or cursor", async () => {
  const first = row("a".repeat(1800));
  const second = { ts: "12", text: "b".repeat(2500) };
  const { slack, calls } = fixture([
    page([first], { response_metadata: { next_cursor: "A" } }),
    page([second], { is_limited: true, has_more: false, response_metadata: { next_cursor: "B" } }),
  ]);
  const stopped = await slack.readInterval("C1", defaults);
  assert.deepEqual(stopped.messages.map((x) => x.message), [first]);
  assert.equal(stopped.history.pages, 1); assert.equal(stopped.history.next_cursor, "A");
  assert.equal(stopped.skipped_non_threadable, 1); assert.deepEqual(stopped.threads, []);
  assert.deepEqual(stopped.coverage_gaps.map((x) => x.kind), ["result_budget"]);
  assert.deepEqual(stopped.continuation, checkpoint("A", ["A"]));
  assert.equal(stopped.requests_made, 2); assert.equal(calls.length, 2); assert.ok(bytes(stopped) <= 4096);
  const resumed = fixture([page([second]), page()]);
  const complete = await resumed.slack.readInterval("C1", { ...defaults, maxResultBytes: 8192, continuation: stopped.continuation });
  assert.equal(new URL(resumed.calls[0].url).searchParams.get("cursor"), "A");
  assert.deepEqual(complete.messages.map((x) => x.message), [second]); assert.equal(complete.continuation, null);
});

test("capacity reserves a concrete provider failure before admitting the preceding page", async () => {
  const first = page([row("a".repeat(3400))], { response_metadata: { next_cursor: "A" } });
  const error = () => new Response(JSON.stringify({ ok: false, error: "team_access_not_granted" }), {
    headers: { "Content-Type": "application/json", "Retry-After": "999999999" } });
  const wide = fixture([first, error()]);
  const observed = await wide.slack.readInterval("C1", { ...defaults, maxResultBytes: 8192 });
  assert.equal(observed.termination, "request_failure"); assert.equal(observed.history.error.retry_after_seconds, 999999999);
  // Only the limit's digit count differs, so this bound is strictly below the real terminal failure.
  const cap = bytes(observed) - 1; assert.ok(cap >= 4096);
  const narrow = fixture([first, error()]);
  const stopped = await narrow.slack.readInterval("C1", { ...defaults, maxResultBytes: cap });
  assert.equal(stopped.termination, "result_budget"); assert.deepEqual(stopped.messages, []);
  assert.deepEqual(stopped.continuation, checkpoint()); assert.equal(narrow.calls.length, 1);
  assert.ok(bytes(stopped) <= cap);
});

test("incoming checkpoint that cannot carry a stop projection refuses before HTTP", async () => {
  const text = "é".repeat(800), input = checkpoint(text, [text]);
  assert.ok(Buffer.byteLength(JSON.stringify(input)) < 4096);
  const { slack, calls } = fixture([]);
  await assert.rejects(slack.readInterval("C1", { ...defaults, continuation: input }), /cannot represent pending work/);
  assert.equal(calls.length, 0); assert.equal(input.pending[0].cursor, text);
});

test("explicit resumes preserve every occurrence and order without clipping", async () => {
  const pages = Array.from({ length: 5 }, (_, i) => page([row(`${i}:` + "é😀".repeat(110)), row(`${i}:duplicate`)],
    i === 4 ? {} : { response_metadata: { next_cursor: String(i + 1) } }));
  const requests = [];
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async (request) => {
    const cursor = new URL(request.url).searchParams.get("cursor"); requests.push(cursor);
    return json(pages[cursor === null ? 0 : Number(cursor)]);
  } });
  const messages = []; let continuation = null, completed = false;
  for (let calls = 0; calls < 8; calls++) {
    const result = await slack.readInterval("C1", { ...defaults, continuation });
    assert.ok(bytes(result) <= 4096); messages.push(...result.messages.map((x) => x.message));
    if (result.continuation === null) { completed = true; break; }
    assert.equal(result.termination, "result_budget"); assert.ok(result.messages.length > 0);
    continuation = result.continuation;
  }
  assert.equal(completed, true); assert.deepEqual(messages, pages.flatMap((x) => x.messages));
  assert.ok(requests.length > pages.length, "refused pages are revisited only by explicit calls");
});

test("non-resumable overflow stays explicit without manufacturing a checkpoint", async () => {
  const { slack } = fixture([page([row("x".repeat(5000))])]);
  const result = await slack.readInterval("C1", { ...defaults, resumable: false });
  assert.equal(result.termination, "result_budget"); assert.equal(Object.hasOwn(result, "continuation"), false);
  assert.deepEqual(result.messages, []); assert.ok(bytes(result) <= 4096);
});

test("budget is captured before await and cancellation still propagates", async () => {
  let release; const wait = new Promise((done) => { release = done; });
  const options = { ...defaults }; let calls = 0;
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async () => {
    calls++; await wait; return json(page([row("x".repeat(5000))]));
  } });
  const pending = slack.readInterval("C1", options); options.maxResultBytes = 8192; release();
  const result = await pending;
  assert.equal(result.limits.max_result_bytes, 4096); assert.equal(result.termination, "result_budget"); assert.equal(calls, 1);
  const abort = new AbortController(); abort.abort();
  await assert.rejects(slack.readInterval("C1", { ...defaults, signal: abort.signal }), SlackCancelled);
  assert.equal(calls, 1);
});


test("an unusual oversized error is not returned and keeps the prior pending step", async () => {
  const { slack } = fixture([]);
  slack.historyPage = async () => { throw new SlackError("provider", 200, "x".repeat(5000)); };
  const result = await slack.readInterval("C1", defaults);
  assert.equal(result.termination, "result_budget"); assert.equal(result.requests_made, 1);
  assert.deepEqual(result.continuation, checkpoint()); assert.equal(Object.hasOwn(result.history, "error"), false);
  assert.ok(bytes(result) <= 4096);
});
