import test from "node:test";
import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { SlackOperations, SlackError, SlackCancelled } from "@rkat/toolkit/connectors/slack";

const json = (value, headers = {}) => new Response(JSON.stringify(value), {
  headers: { "Content-Type": "application/json", ...headers },
});
const user = (id, profile = {}) => ({ ok: true, user: { id, profile } });
const failure = (kind, code = null) => (error) => {
  assert.ok(error instanceof SlackError);
  assert.equal(error.kind, kind);
  assert.equal(error.providerCode, code);
  assert.ok(!String(error).includes("private-canary"));
  return true;
};

test("user lookup preserves exact optional data and selected account over real loopback", async () => {
  const calls = [];
  const server = http.createServer((request, response) => {
    calls.push({ url: request.url, method: request.method, auth: request.headers.authorization });
    const id = new URL(request.url, "http://fixture/").searchParams.get("user");
    response.writeHead(200, { "Content-Type": "application/json", Connection: "close" });
    response.end(JSON.stringify({ ok: true, user: { id, team_id: "TFOREIGN", deleted: false,
      profile: { display_name: "", real_name: null }, future: { selected: request.headers.authorization } },
      response_metadata: { warnings: ["preserved"] } }));
  });
  server.listen(0, "127.0.0.1"); await once(server, "listening");
  const baseUrl = `http://127.0.0.1:${server.address().port}/prefix/`;
  const binding = (account) => ({ baseUrl, fetch: (request) => {
    assert.equal(request.redirect, "manual"); assert.equal(request.credentials, "omit");
    const headers = new Headers(request.headers); headers.set("Authorization", account);
    return fetch(new Request(request, { headers }));
  } });
  try {
    const first = new SlackOperations(binding("first"), { allowedChannelIds: [] });
    const second = new SlackOperations(binding("second"));
    const result = await first.getUser("W123", { includeLocale: true });
    assert.deepEqual(result.user.profile, { display_name: "", real_name: null });
    assert.equal(result.user.team_id, "TFOREIGN");
    assert.equal(result.user.future.selected, "first");
    assert.equal((await second.getUser("W123")).user.future.selected, "second");
    await first.getUser("W123");
    assert.deepEqual(calls, [
      { url: "/prefix/api/users.info?user=W123&include_locale=true", method: "GET", auth: "first" },
      { url: "/prefix/api/users.info?user=W123&include_locale=false", method: "GET", auth: "second" },
      { url: "/prefix/api/users.info?user=W123&include_locale=false", method: "GET", auth: "first" },
    ]);
    assert.deepEqual(result.response_metadata, { warnings: ["preserved"] });
  } finally { await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())); }
});

test("user input and flags are validated before transport", async () => {
  const slack = new SlackOperations({ baseUrl: "https://slack.com/", fetch: () => assert.fail("transport") });
  for (const id of ["", "U/12", "<@U12>", "U12\n", "U".repeat(129), null, 3]) {
    await assert.rejects(slack.getUser(id), TypeError);
  }
  for (const includeLocale of [null, 1, "false"]) {
    await assert.rejects(slack.getUser("U12", { includeLocale }), TypeError);
  }
  const controller = new AbortController(); controller.abort("private-canary");
  await assert.rejects(slack.getUser("U12", { signal: controller.signal }), SlackCancelled);
});

test("exact returned identity is required but optional profile data is not invented", async () => {
  for (const value of [user("U2"), { ok: true }, { ok: true, user: null },
    { ok: true, user: [] }, { ok: true, user: { id: 12 } }]) {
    const slack = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async () => json(value) });
    await assert.rejects(slack.getUser("U1"), failure("invalid_response"));
  }
  for (const value of [{ ok: true, user: { id: "U1" } },
    { ok: true, user: { id: "U1", profile: null, deleted: true } }]) {
    const slack = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async () => json(value) });
    assert.deepEqual(await slack.getUser("U1"), value);
  }
});

test("missing user and denied scope stay explicit and do not retry", async () => {
  for (const [code, kind] of [["user_not_found", "not_found"], ["missing_scope", "permission"],
    ["ratelimited", "rate_limited"], ["private-canary", "provider"]]) {
    let calls = 0;
    const slack = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async () => {
      calls++; return json({ ok: false, error: code, needed: "private-canary" }, { "Retry-After": "9" });
    } });
    await assert.rejects(slack.getUser("U1"), failure(kind, code === "private-canary" ? null : code));
    assert.equal(calls, 1);
  }
});

test("user response obeys existing byte bounds and cleanup", async () => {
  let cancelled = 0;
  const slack = new SlackOperations({ baseUrl: "https://slack.com/", fetch: async () => new Response(
    new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(1025)); },
      cancel() { cancelled++; } }), { headers: { "Content-Type": "application/json" } }) },
  { maxResponseBytes: 1024 });
  await assert.rejects(slack.getUser("U1"), failure("response_too_large"));
  assert.equal(cancelled, 1);
});
