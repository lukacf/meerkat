import test from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { SlackOperations, SlackCancelled } from "@rkat/toolkit/connectors/slack";

const defaults = { start: "10", end: "20", rootOldest: "0", maxRequests: 10, maxMessages: 20, resumable: true };
const selection = { channel: "C1", start: "10", end: "20", root_oldest: "0", page_limit: 15, include_all_metadata: false };
const page = (messages = [], extra = {}) => ({ ok: true, messages, ...extra });
const step = (root_ts = null, cursor = null, seen_cursors = []) => ({ root_ts, cursor, seen_cursors });
const checkpoint = (pending = [step()], discovered_roots = []) => ({ schema_version: 1, selection: { ...selection }, pending, discovered_roots });
const json = (value) => new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });
function fixture(pages, account = "first") {
  const calls = [];
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async (request) => {
    calls.push({ request, account }); const value = pages[calls.length - 1];
    assert.notEqual(value, undefined, "unexpected additional request");
    return value instanceof Response ? value : json(value);
  } });
  return { slack, calls };
}
const cursorAt = (calls, index) => new URL(calls[index].request.url).searchParams.get("cursor");
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { resolve, promise }; };
async function bounded(promise) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error("fixture did not join")), 2000); })]); }
  finally { clearTimeout(timer); }
}

test("one-shot shape unchanged; opt-in terminal result is explicitly current-call-only", async () => {
  const { slack } = fixture([page(), page()]);
  const legacy = await slack.readInterval("C1", { ...defaults, resumable: false, continuation: null });
  assert.equal(Object.hasOwn(legacy, "continuation"), false);
  assert.equal(Object.hasOwn(legacy, "observation_scope"), false);
  const result = await slack.readInterval("C1", defaults);
  assert.equal(result.observation_scope, "current_call_only"); assert.equal(result.continuation, null);
  assert.equal(result.history.state, "exhausted");
});

test("FIFO across calls retains exact cursors and deduplicates discovered roots only", async () => {
  const child = { ts: "11", thread_ts: "1", text: "first child" }, next = { ts: "12", thread_ts: "1", text: "second child" };
  const pages = [page([{ ts: "1" }], { response_metadata: { next_cursor: "H" } }),
    page([child, child], { response_metadata: { next_cursor: "R" } }), page([{ ts: "1" }]), page([next])];
  const one = fixture(pages), complete = await one.slack.readInterval("C1", defaults), resumed = fixture(pages);
  const first = await resumed.slack.readInterval("C1", { ...defaults, maxRequests: 1 });
  assert.deepEqual(first.continuation, checkpoint([step("1"), step(null, "H", ["H"])], ["1"]));
  const second = await resumed.slack.readInterval("C1", { ...defaults, maxRequests: 2, continuation: first.continuation });
  assert.deepEqual(second.continuation, checkpoint([step("1", "R", ["R"])], ["1"]));
  const third = await resumed.slack.readInterval("C1", { ...defaults, maxRequests: 1, continuation: second.continuation });
  assert.deepEqual(third.history, { state: "not_requested", pages: 0, stop_reason: null });
  assert.deepEqual(third.messages.map((row) => row.message), [next]);
  assert.deepEqual([...first.messages, ...second.messages, ...third.messages], complete.messages);
  assert.deepEqual(resumed.calls.map(({ request }) => request.url), one.calls.map(({ request }) => request.url));
  assert.equal(third.continuation, null); assert.deepEqual(third.threads.map((x) => [x.root_ts, x.pages]), [["1", 1]]);
});

test("rate limit has no implicit retry and preserves failed step for an explicit later call", async () => {
  const input = checkpoint([step("1", "R", ["R"]), step(null, "H", ["H"])], ["1"]);
  const { slack, calls } = fixture([new Response("", { status: 429, headers: { "Retry-After": "60" } }), page(), page()]);
  const result = await slack.readInterval("C1", { ...defaults, continuation: input });
  assert.equal(calls.length, 1); assert.equal(result.termination, "request_failure");
  assert.equal(result.threads[0].error.retry_after_seconds, 60); assert.deepEqual(result.continuation, input);
  const completed = await slack.readInterval("C1", { ...defaults, continuation: result.continuation });
  assert.deepEqual(calls.map((_, i) => cursorAt(calls, i)), ["R", "R", "H"]); assert.equal(completed.continuation, null);
});

test("message refusal preserves the page; a later larger budget admits it once", async () => {
  const input = checkpoint([step("1", "R", ["R"])], ["1"]);
  const rows = [{ ts: "11", thread_ts: "1" }, { ts: "12", thread_ts: "1" }];
  const { slack, calls } = fixture([page(rows, { is_limited: true }), page(rows)]);
  const failed = await slack.readInterval("C1", { ...defaults, continuation: input, maxMessages: 1 });
  assert.equal(failed.termination, "message_budget"); assert.deepEqual(failed.messages, []);
  assert.equal(failed.threads[0].pages, 0); assert.deepEqual(failed.continuation, input);
  assert.deepEqual(failed.coverage_gaps.map((gap) => gap.kind), ["message_budget"]);
  const result = await slack.readInterval("C1", { ...defaults, continuation: failed.continuation, maxMessages: 2 });
  assert.deepEqual(result.messages.map((row) => row.message), rows);
  assert.deepEqual(calls.map((_, i) => cursorAt(calls, i)), ["R", "R"]);
});

test("remembered cursor cycles and unavailable threads remain terminal for that chain", async () => {
  const first = fixture([page([], { response_metadata: { next_cursor: "A" } })]);
  const result = await first.slack.readInterval("C1", { ...defaults, continuation: checkpoint([step("1", "B", ["A", "B"])], ["1"]) });
  assert.equal(result.threads[0].stop_reason, "cursor_cycle"); assert.equal(result.continuation, null); assert.equal(first.calls.length, 1);
  const second = fixture([{ ok: false, error: "thread_not_found" }]);
  const stopped = await second.slack.readInterval("C1", { ...defaults, maxRequests: 1, continuation: checkpoint([step("1"), step("2")], ["1", "2"]) });
  assert.equal(stopped.threads[0].stop_reason, "thread_not_found");
  assert.deepEqual(stopped.continuation, checkpoint([step("2")], ["1", "2"]));
});

test("all nested continuation collections are captured before first await", async () => {
  const entered = deferred(), release = deferred(), calls = [];
  const input = checkpoint([step("1", "R", ["R"]), step(null, "H", ["H"])], ["1"]);
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async (request) => {
    calls.push({ request }); if (calls.length === 1) { entered.resolve(); await release.promise; } return json(page());
  } });
  const operation = slack.readInterval("C1", { ...defaults, continuation: input });
  await bounded(entered.promise); input.selection.channel = "C2"; input.pending[1].cursor = "attacker";
  input.pending[1].seen_cursors.push("attacker"); input.pending.shift(); input.discovered_roots.push("17");
  release.resolve(); const result = await bounded(operation);
  assert.equal(result.channel, "C1"); assert.equal(cursorAt(calls, 1), "H"); assert.equal(result.continuation, null);
});

test("checkpoint cannot select a different account or manufacture prior observations", async () => {
  const input = checkpoint([step("1")], ["1"]);
  for (const account of ["one", "two"]) {
    const { slack, calls } = fixture([page()], account);
    const result = await slack.readInterval("C1", { ...defaults, continuation: input });
    assert.deepEqual(result.messages, []); assert.equal(result.threads[0].pages, 1);
    assert.equal(result.observation_scope, "current_call_only"); assert.equal(calls[0].account, account);
    assert.equal(new URL(calls[0].request.url).searchParams.get("ts"), "1");
  }
});

test("malformed checkpoints and mismatched selections refuse before fetching or invoking getters", async () => {
  const malformed = [[], true, "opaque", { ...checkpoint(), unknown: null }];
  const change = (mutate) => { const value = checkpoint(); mutate(value); malformed.push(value); };
  for (const [key, value] of [["channel", "C2"], ["start", "10.0"], ["end", "21"], ["root_oldest", "1"],
    ["page_limit", true], ["include_all_metadata", 0], ["extra", "no"]]) change((x) => { x.selection[key] = value; });
  change((x) => { x.schema_version = true; }); change((x) => { x.pending = []; });
  change((x) => { x.pending.push(step()); }); change((x) => { x.pending[0].root_ts = "1"; });
  change((x) => { x.pending[0].extra = "no"; }); change((x) => { x.pending[0].cursor = "x"; });
  change((x) => { x.pending[0].seen_cursors = ["x"]; });
  for (const [selected, seen] of [["x", ["x", "x"]], ["x", ["x", "y"]], ["bad\n", ["bad\n"]],
    ["\ud800", ["\ud800"]], ["x".repeat(8193), ["x".repeat(8193)]]]) change((x) => { x.pending[0] = step(null, selected, seen); });
  for (const roots of [["20"], ["1", "1"], [true], Array.from({ length: 1025 }, (_, i) => `0.${i}`)]) change((x) => { x.discovered_roots = roots; });
  change((x) => { x.pending = Array.from({ length: 1026 }, () => step()); });
  change((x) => { x.pending[0] = step(null, "2048", Array.from({ length: 2049 }, (_, i) => String(i))); });
  change((x) => {
    const seen = Array.from({ length: 1025 }, (_, i) => String(i));
    x.discovered_roots = ["1", "2"];
    x.pending = [step("1", seen.at(-1), seen), step("2", seen.at(-1), [...seen])];
  });
  change((x) => { x.pending[0] = step(null, "Z", [...Array.from({ length: 40 }, (_, i) => "x".repeat(8190) + i), "Z"]); });
  let getterCalls = 0;
  change((x) => { Object.defineProperty(x, "pending", { get() { getterCalls++; return []; }, enumerable: true }); });
  change((x) => { x.pending = new Array(1); }); change((x) => { x.pending.extra = true; });
  change((x) => { x.pending[0].seen_cursors = ["x"]; x.pending[0].seen_cursors[Symbol("extra")] = true; });
  const { slack, calls } = fixture([]);
  for (const continuation of malformed) await assert.rejects(slack.readInterval("C1", { ...defaults, continuation }), TypeError);
  await assert.rejects(slack.readInterval("C1", { ...defaults, resumable: false, continuation: checkpoint() }), TypeError);
  await assert.rejects(slack.readInterval("C1", { ...defaults, resumable: 1 }), TypeError);
  assert.equal(calls.length, 0); assert.equal(getterCalls, 0);
});

test("root checkpoint bound refuses all page projections and keeps pre-page checkpoint", async () => {
  const input = checkpoint([step()], Array.from({ length: 1024 }, (_, i) => `0.${i}`));
  const { slack } = fixture([page([{ ts: "11" }], { is_limited: true, response_metadata: { next_cursor: "new" } })]);
  const result = await slack.readInterval("C1", { ...defaults, continuation: input });
  assert.equal(result.termination, "continuation_budget"); assert.equal(result.history.stop_reason, "continuation_budget");
  assert.equal(result.history.pages, 0); assert.deepEqual(result.messages, []); assert.deepEqual(result.threads, []);
  assert.deepEqual(result.coverage_gaps.map((gap) => gap.kind), ["continuation_budget"]); assert.deepEqual(result.continuation, input);
});

test("aggregate cursor bound refuses nonterminal page but terminal page needs no checkpoint", async () => {
  const seen = Array.from({ length: 2048 }, (_, i) => `cursor-${i}`), input = checkpoint([step(null, seen.at(-1), seen)]);
  const { slack, calls } = fixture([page([], { response_metadata: { next_cursor: "next" } }), page()]);
  const result = await slack.readInterval("C1", { ...defaults, continuation: input });
  assert.equal(result.termination, "continuation_budget"); assert.deepEqual(result.continuation, input);
  const finished = await slack.readInterval("C1", { ...defaults, continuation: result.continuation });
  assert.equal(finished.continuation, null); assert.equal(finished.history.pages, 1); assert.equal(cursorAt(calls, 0), cursorAt(calls, 1));
});

test("byte cap counts compact UTF-8 including duplicated active cursor", async () => {
  const seen = Array.from({ length: 30 }, (_, i) => "ö".repeat(4093) + String(i).padStart(3, "0"));
  const input = checkpoint([step(null, seen.at(-1), seen)]);
  assert.ok(Buffer.byteLength(JSON.stringify(input)) < 262144);
  assert.ok(Buffer.byteLength(JSON.stringify(checkpoint([step(null, "x".repeat(8190), [...seen, "x".repeat(8190)])]))) > 262144);
  const { slack } = fixture([page([], { response_metadata: { next_cursor: "x".repeat(8190) } })]);
  const result = await slack.readInterval("C1", { ...defaults, maxResultBytes: 1048576, continuation: input });
  assert.equal(result.termination, "continuation_budget"); assert.deepEqual(result.continuation, input);
});

test("cancellation joins response cleanup and never becomes resumable success", async () => {
  const entered = deferred(), release = deferred(), closing = deferred(), closed = deferred();
  const abort = new AbortController(), input = checkpoint([step("1")], ["1"]), original = structuredClone(input);
  let settled = false, fetches = 0, cancels = 0;
  const slack = new SlackOperations({ baseUrl: "https://slack.invalid/", fetch: async () => {
    fetches++; entered.resolve(); await release.promise;
    return new Response(new ReadableStream({ cancel: async () => { cancels++; closing.resolve(); await closed.promise; } }));
  } });
  const operation = slack.readInterval("C1", { ...defaults, continuation: input, signal: abort.signal })
    .then((value) => ({ value }), (error) => ({ error })).finally(() => { settled = true; });
  try {
    await bounded(entered.promise); abort.abort("private reason"); await delay(5); assert.equal(settled, false);
    release.resolve(); await bounded(closing.promise); await delay(5); assert.equal(settled, false);
    closed.resolve(); const outcome = await bounded(operation);
    assert.ok(outcome.error instanceof SlackCancelled); assert.equal(outcome.value, undefined);
    assert.deepEqual(input, original); assert.equal(fetches, 1); assert.equal(cancels, 1);
  } finally { release.resolve(); closed.resolve(); await bounded(operation); }
});

test("legacy large valid page is admitted without variadic argument limits", async () => {
  const count = 140000;
  const { slack, calls } = fixture([page(Array.from({ length: count }, () => ({ ts: "11" })))]);
  const result = await slack.readInterval("C1", { ...defaults, resumable: false, maxRequests: 1, maxMessages: count, maxResultBytes: 16777216 });
  assert.equal(result.messages.length, count); assert.equal(result.history.pages, 1);
  assert.equal(result.threads.length, 1); assert.equal(calls.length, 1);
  assert.equal(Object.hasOwn(result, "continuation"), false);
});
