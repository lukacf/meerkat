// Controlled provider replies exercise the real public request and cleanup path.
import test from "node:test";
import assert from "node:assert/strict";
import { GmailOperations, GmailError, GmailCancelled } from "@rkat/toolkit/connectors/gmail";

const secret = "private-token-message-provider-detail";
const reasons = [
  ["usageLimits", "rateLimitExceeded", "rate_limited"],
  ["usageLimits", "userRateLimitExceeded", "rate_limited"],
  ["usageLimits", "dailyLimitExceeded", "quota_exceeded"],
  ["global", "domainPolicy", "permission"],
];
const body = (domain = "usageLimits", reason = "rateLimitExceeded") => ({
  error: { code: 403, message: secret, errors: [{ domain, reason, message: secret }] },
});
const mime = new TextEncoder().encode("To: fixture@example.test\r\n\r\n" + secret);

function fixture(reply, options = {}) {
  const requests = [];
  const operations = new GmailOperations({
    baseUrl: "https://gmail.googleapis.com/", userId: "selected@example.test",
    fetch: async request => { requests.push(request); return reply(request); },
  }, options);
  return { operations, requests };
}

async function rejected(operation, kind, reason = null, outcome = "unknown", status = 403) {
  await assert.rejects(operation, error => {
    assert.ok(error instanceof GmailError);
    assert.equal(error.kind, kind);
    assert.equal(error.providerReason, reason);
    assert.equal(error.writeOutcome, outcome);
    assert.equal(error.statusCode, status);
    assert.equal(error.cause, undefined);
    assert.ok(!JSON.stringify(error).includes(secret));
    assert.ok(!String(error).includes(secret));
    return true;
  });
}

for (const [domain, reason, kind] of reasons) {
  test(`Gmail 403 ${reason} identifies the failure without resending`, async () => {
    const f = fixture(() => new Response(JSON.stringify(body(domain, reason)), {
      status: 403, headers: { "Content-Type": "application/json", "Retry-After": "17" },
    }));
    await rejected(f.operations.sendRaw(mime), kind, reason);
    assert.equal(f.requests.length, 1);
    assert.equal(f.requests[0].method, "POST");
    assert.equal(new URL(f.requests[0].url).pathname,
      "/gmail/v1/users/selected%40example.test/messages/send");
    await assert.rejects(f.operations.listMessagesPage(), error => {
      assert.equal(error.kind, kind);
      assert.equal(error.providerReason, reason);
      assert.equal(error.writeOutcome, "not_applicable");
      assert.equal(error.retryAfterSeconds, 17);
      return true;
    });
    assert.equal(f.requests.length, 2);
  });
}

test("Gmail accepts repeated identical reasons but not mixed or malformed evidence", async () => {
  const repeated = body();
  repeated.error.errors.push({ ...repeated.error.errors[0] });
  await rejected(fixture(() => Response.json(repeated, { status: 403 })).operations.sendRaw(mime),
    "rate_limited", "rateLimitExceeded");
  const bad = [
    {}, { error: null }, { error: { code: "403", errors: body().error.errors } },
    { error: { code: 429, errors: body().error.errors } },
    { error: { code: 403, errors: [] } },
    { error: { code: 403, errors: Array(33).fill(body().error.errors[0]) } },
    body("global"), body("usageLimits", secret), body("usageLimits", "domainPolicy"),
    { error: { code: 403, errors: [body().error.errors[0], body("usageLimits", "userRateLimitExceeded").error.errors[0]] } },
    { error: { code: 403, errors: [body().error.errors[0], null] } },
  ];
  const raw = ["not json", '{"error":{"code":403,"code":403,"errors":[]}}',
    '{"error":{"code":403,"errors":[{"domain":"usageLimits","reason":"rateLimitExceeded","reason":"domainPolicy"}]}}',
    JSON.stringify(body()).replace('"message":"' + secret + '"', '"message":NaN'),
    JSON.stringify(body()).replace('"code":403', '"code":403,"deep":' + '['.repeat(130) + '0' + ']'.repeat(130)),
    ...bad.map(value => JSON.stringify(value))];
  for (const payload of raw) {
    const f = fixture(() => new Response(payload, {
      status: 403, headers: { "Content-Type": "application/json" },
    }));
    await rejected(f.operations.sendRaw(mime), "permission");
    assert.equal(f.requests.length, 1);
  }
});

test("Gmail 403 malformed encodings and lengths retain the generic HTTP failure", async () => {
  for (const headers of [
    { "Content-Type": "text/plain" },
    { "Content-Type": "application/json", "Content-Encoding": "gzip" },
    { "Content-Type": "application/json", "Content-Length": "65537" },
    { "Content-Type": "application/json", "Content-Length": "2" },
    { "Content-Type": "application/json", "Content-Length": "-1" },
  ]) {
    await rejected(fixture(() => new Response(JSON.stringify(body()), { status: 403, headers }))
      .operations.sendRaw(mime), "permission");
  }
  const invalidUtf8 = new Uint8Array([...new TextEncoder().encode(JSON.stringify(body())), 0xff]);
  await rejected(fixture(() => new Response(invalidUtf8, {
    status: 403, headers: { "Content-Type": "application/json" },
  })).operations.sendRaw(mime), "permission");
});

test("Gmail limits error bytes by both the diagnostic cap and configured bound", async () => {
  for (const limit of [64, 100000]) {
    const value = body();
    value.error.message = "x".repeat(66000);
    let pulls = 0, closed = false;
    const bytes = new TextEncoder().encode(JSON.stringify(value));
    const f = fixture(() => new Response(new ReadableStream({
      pull(controller) {
        const start = pulls++ * 1024;
        if (start >= bytes.length) controller.close();
        else controller.enqueue(bytes.slice(start, start + 1024));
      },
      cancel() { closed = true; },
    }, { highWaterMark: 0 }), { status: 403, headers: { "Content-Type": "application/json" } }),
    { maxResponseBytes: limit });
    await rejected(f.operations.sendRaw(mime), "permission");
    assert.ok(pulls <= (limit === 64 ? 1 : 65));
    assert.equal(closed, true);
  }
});

test("Gmail interrupted diagnostic reads preserve the received HTTP refusal", async () => {
  const f = fixture(() => new Response(new ReadableStream({
    pull(controller) { controller.error(new Error(secret)); },
  }), { status: 403, headers: { "Content-Type": "application/json" } }));
  await rejected(f.operations.sendRaw(mime), "permission");
  assert.equal(f.requests.length, 1);
});

test("Gmail diagnostic cancellation joins stream cleanup and preserves write uncertainty", async () => {
  const abort = new AbortController();
  let enter, release, closed = false, settled = false;
  const entered = new Promise(resolve => { enter = resolve; });
  const cleanup = new Promise(resolve => { release = resolve; });
  const f = fixture(() => new Response(new ReadableStream({
    pull() { enter(); },
    async cancel() { await cleanup; closed = true; },
  }, { highWaterMark: 0 }), { status: 403, headers: { "Content-Type": "application/json" } }));
  const pending = f.operations.sendRaw(mime, { signal: abort.signal });
  pending.then(() => { settled = true; }, () => { settled = true; });
  await entered;
  abort.abort(secret);
  await new Promise(resolve => setTimeout(resolve, 10));
  assert.equal(settled, false);
  release();
  await assert.rejects(pending, error => error instanceof GmailCancelled
    && error.writeOutcome === "unknown" && !String(error).includes(secret));
  assert.equal(closed, true);
  assert.equal(f.requests.length, 1);
});

test("Gmail error reasons do not alter non-403 status handling", async () => {
  for (const [status, kind] of [[401, "authentication"], [429, "rate_limited"], [500, "http_status"]]) {
    const f = fixture(() => Response.json(body(), { status }));
    await rejected(f.operations.sendRaw(mime), kind, null, "unknown", status);
  }
});

test("Gmail diagnostic reads keep the original total deadline", async () => {
  let closed = false;
  const f = fixture(() => new Response(new ReadableStream({
    cancel() { closed = true; },
  }), { status: 403, headers: { "Content-Type": "application/json" } }), { timeoutMs: 20 });
  await rejected(f.operations.sendRaw(mime), "timeout", null, "unknown", null);
  assert.equal(closed, true);
  assert.equal(f.requests.length, 1);
});
