import test from "node:test";
import assert from "node:assert/strict";
import { SlackOperations, SlackError } from "@rkat/toolkit/connectors/slack";

const interval = { start: "10", end: "20", rootOldest: "0", maxRequests: 1, maxMessages: 20, resumable: true };
function fixture(options = {}, account = "one", respond) {
  const calls = [];
  const context = { baseUrl: "https://slack.invalid/", fetch: async (request) => {
    calls.push({ request, account });
    const url = new URL(request.url);
    let value;
    if (respond) value = respond(url);
    else if (url.pathname.endsWith("auth.test")) value = { ok: true, team_id: "T1", user_id: "U1" };
    else if (url.pathname.endsWith("conversations.list")) value = { ok: true,
      channels: [{ id: "C2", name: "other" }], response_metadata: { next_cursor: "all-channels" } };
    else if (url.pathname.endsWith("conversations.info")) value = { ok: true, channel: { id: url.searchParams.get("channel") } };
    else value = { ok: true, messages: [] };
    return new Response(JSON.stringify(value), { headers: { "Content-Type": "application/json" } });
  } };
  return { slack: new SlackOperations(context, options), calls, context };
}
function permission(error) {
  assert.ok(error instanceof SlackError);
  assert.equal(error.kind, "permission"); assert.equal(error.message, "Slack permission");
  assert.equal(error.statusCode, null); assert.equal(error.providerCode, null);
  assert.equal(error.retryAfterSeconds, null); assert.equal(error.cause, undefined);
  return true;
}

test("restricted channel calls and broad listing refuse before host fetch", async () => {
  for (const allowedChannelIds of [[], ["C1"]]) {
    const { slack, calls } = fixture({ allowedChannelIds });
    for (const call of [() => slack.historyPage("C2"), () => slack.repliesPage("C2", "1"),
      () => slack.getConversation("C2"), () => slack.readInterval("C2", interval),
      () => slack.listConversationsPage(), () => slack.listConversationsPage({ cursor: "previous-unrestricted-cursor" })]) {
      await assert.rejects(call, permission);
    }
    assert.equal(calls.length, 0);
    assert.equal((await slack.authTest()).team_id, "T1"); assert.equal(calls.length, 1);
  }
});

test("omitted and undefined preserve unrestricted calls and exact one-shot shape", async () => {
  for (const options of [{}, { allowedChannelIds: undefined }]) {
    const { slack, calls } = fixture(options);
    assert.equal((await slack.getConversation("C2")).channel.id, "C2");
    assert.deepEqual((await slack.historyPage("C2")).messages, []);
    assert.deepEqual((await slack.repliesPage("C2", "1")).messages, []);
    const old = await slack.readInterval("C2", { ...interval, resumable: false });
    assert.equal(Object.hasOwn(old, "continuation"), false);
    assert.equal(Object.hasOwn(old, "observation_scope"), false);
    assert.equal((await slack.listConversationsPage()).response_metadata.next_cursor, "all-channels");
    assert.equal(calls.length, 5);
  }
});

test("membership is copied, exact and independent across host-selected accounts", async () => {
  const allowedChannelIds = ["C1"];
  const one = fixture({ allowedChannelIds }, "one"), two = fixture({ allowedChannelIds: ["C2"] }, "two");
  allowedChannelIds.splice(0, 1, "C2");
  await one.slack.historyPage("C1"); await two.slack.historyPage("C2");
  await assert.rejects(one.slack.historyPage("C2"), permission);
  await assert.rejects(one.slack.historyPage("c1"), permission);
  await assert.rejects(two.slack.historyPage("C1"), permission);
  assert.equal(one.calls.length, 1); assert.equal(two.calls.length, 1);
  assert.equal(one.calls[0].account, "one"); assert.equal(two.calls[0].account, "two");
});

test("resumption rechecks the current ceiling before using earlier query data", async () => {
  const respond = (url) => ({ ok: true, messages: url.pathname.endsWith("conversations.history")
    ? [{ ts: "1" }] : [{ ts: "11", thread_ts: "1", text: "A quote mentioning C2 is preserved" }] });
  const old = fixture({}, "one", respond), narrowed = fixture({ allowedChannelIds: ["C1"] }, "one", respond);
  const previous = await old.slack.readInterval("C2", interval), checkpoint = structuredClone(previous.continuation);
  await assert.rejects(narrowed.slack.readInterval("C2", { ...interval, continuation: checkpoint }), permission);
  await assert.rejects(narrowed.slack.readInterval("C1", { ...interval, continuation: checkpoint }), TypeError);
  assert.equal(narrowed.calls.length, 0); assert.deepEqual(checkpoint, previous.continuation);
  const first = await narrowed.slack.readInterval("C1", interval);
  const last = await narrowed.slack.readInterval("C1", { ...interval, continuation: first.continuation });
  assert.equal(first.termination, "request_budget"); assert.equal(last.continuation, null);
  assert.equal(last.history.state, "not_requested");
  assert.equal(last.messages[0].message.text, "A quote mentioning C2 is preserved");
  assert.deepEqual(narrowed.calls.map(({ request }) => new URL(request.url).searchParams.get("channel")), ["C1", "C1"]);
});

test("invalid, oversized and non-data arrays refuse without invoking element getters", async () => {
  const { context, calls } = fixture(); let getterCalls = 0;
  const getter = []; Object.defineProperty(getter, "0", { get() { getterCalls++; return "C1"; }, enumerable: true });
  const extra = ["C1"]; extra.extra = true;
  const invalid = [null, true, "C1", new Set(["C1"]), {}, [null], [true], [1], [""], ["C1\n"],
    ["C-1"], ["C".repeat(129)], ["C1", "C1"], Array.from({ length: 257 }, (_, i) => `C${i}`),
    new Array(1), getter, extra];
  for (const allowedChannelIds of invalid) {
    assert.throws(() => new SlackOperations(context, { allowedChannelIds }), { name: "TypeError",
      message: "allowedChannelIds must contain at most 256 unique Slack identifiers" });
  }
  assert.equal(calls.length, 0); assert.equal(getterCalls, 0);
  await new SlackOperations(context, { allowedChannelIds: Object.freeze(Array.from({ length: 256 }, (_, i) => `C${i}`)) })
    .historyPage("C255");
  await new SlackOperations(context, { allowedChannelIds: ["C".repeat(128), "C1", "c1"] }).historyPage("C".repeat(128));
  assert.equal(calls.length, 2);
});

test("recognized conversation response identity mismatch remains a fixed failure", async () => {
  const { slack, calls } = fixture({ allowedChannelIds: ["C1"] }, "one",
    () => ({ ok: true, channel: { id: "C2", name: "foreign-secret" } }));
  await assert.rejects(slack.getConversation("C1"), (error) => {
    assert.equal(error.kind, "invalid_response");
    assert.equal(error.message.includes("foreign"), false); assert.equal(error.message.includes("C2"), false);
    return true;
  });
  assert.equal(calls.length, 1);
});
