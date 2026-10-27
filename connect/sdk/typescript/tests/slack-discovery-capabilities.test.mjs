import test from "node:test";
import assert from "node:assert/strict";
import { SlackOperations } from "@rkat/toolkit/connectors/slack";

const response = (value) => new Response(JSON.stringify(value), {
  headers: { "Content-Type": "application/json" },
});

test("authTest observes selected identity through one empty POST", async () => {
  const requests = [];
  const value = { ok: true, team_id: "T1", user_id: "U1", future: [null, false] };
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async (request) => {
    requests.push(request);
    return response(value);
  } });
  assert.deepEqual(await slack.authTest(), value);
  assert.equal(requests.length, 1);
  assert.equal(requests[0].method, "POST");
  assert.equal(requests[0].url, "https://selected.example.test/api/auth.test");
  assert.equal(await requests[0].text(), "");
});

test("listConversationsPage retains empty nonterminal pages without traversal", async () => {
  const requests = [];
  const value = { ok: true, channels: [], response_metadata: { next_cursor: "opaque+/= next", future: null } };
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async (request) => {
    requests.push(request);
    return response(value);
  } });
  assert.deepEqual(await slack.listConversationsPage({ types: ["im", "private_channel"], teamId: "T1" }), value);
  assert.equal(requests.length, 1);
  assert.deepEqual(Object.fromEntries(new URL(requests[0].url).searchParams), {
    types: "im,private_channel", exclude_archived: "false", limit: "100", team_id: "T1",
  });
});

test("getConversation preserves selected ID and unknown metadata", async () => {
  const requests = [];
  const value = { ok: true, channel: { id: "C1", future: { permitted: false, subject: null } } };
  const slack = new SlackOperations({ baseUrl: "https://selected.example.test/", fetch: async (request) => {
    requests.push(request);
    return response(value);
  } });
  assert.deepEqual(await slack.getConversation("C1", { includeLocale: true }), value);
  assert.equal(requests.length, 1);
  assert.deepEqual(Object.fromEntries(new URL(requests[0].url).searchParams), {
    channel: "C1", include_locale: "true", include_num_members: "false",
  });
});
