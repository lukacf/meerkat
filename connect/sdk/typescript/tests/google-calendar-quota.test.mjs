// Real public request/cleanup path with controlled provider responses only.
import test from "node:test";
import assert from "node:assert/strict";
import { GoogleCalendarOperations, CalendarError, CalendarCancelled } from "@rkat/toolkit/connectors/google-calendar";

const secret = "private-quota-token-and-provider-prose";
const body = (reason = "rateLimitExceeded", domain = "usageLimits") => ({
  error: { code: 403, message: secret, errors: [{ domain, reason, message: secret }] },
});
function fixture(reply, options = {}) {
  const requests = [];
  const operations = new GoogleCalendarOperations({
    baseUrl: "https://www.googleapis.com/", calendarId: "selected",
    fetch: async request => { requests.push(request); return reply(request); },
  }, options);
  return { operations, requests };
}
async function rejected(operation, kind, outcome = "unknown", status = 403) {
  await assert.rejects(operation, error => {
    assert.ok(error instanceof CalendarError);
    assert.equal(error.kind, kind);
    assert.equal(error.writeOutcome, outcome);
    assert.equal(error.statusCode, status);
    assert.equal(error.cause, undefined);
    assert.ok(!JSON.stringify(error).includes(secret));
    assert.ok(!String(error).includes(secret));
    return true;
  });
}

for (const reason of ["rateLimitExceeded", "userRateLimitExceeded"]) {
  test(`Calendar 403 ${reason} preserves rate limit and one attempt`, async () => {
    const f = fixture(() => Response.json(body(reason), {
      status: 403, headers: { "Retry-After": "17" },
    }));
    await rejected(f.operations.deleteEvent("selected"), "rate_limited");
    assert.equal(f.requests.length, 1);
    assert.equal(f.requests[0].method, "DELETE");
    await assert.rejects(f.operations.getEvent("selected"), error => {
      assert.equal(error.kind, "rate_limited");
      assert.equal(error.writeOutcome, "not_applicable");
      assert.equal(error.retryAfterSeconds, 17);
      return true;
    });
    assert.equal(f.requests.length, 2);
  });
}

test("Calendar repeated consistent evidence is accepted, conflicts and malformed evidence are not", async () => {
  const repeated = body();
  repeated.error.errors = Array(32).fill(repeated.error.errors[0]);
  await rejected(fixture(() => Response.json(repeated, { status: 403 })).operations.deleteEvent("selected"), "rate_limited");
  const bad = [
    {}, { error: null }, { error: { code: "403", errors: body().error.errors } },
    { error: { code: 429, errors: body().error.errors } },
    { error: { code: 403, errors: [] } },
    { error: { code: 403, errors: Array(33).fill(body().error.errors[0]) } },
    body("rateLimitExceeded", "global"), body(secret), body("quotaExceeded"),
    { error: { code: 403, errors: [body().error.errors[0], body("userRateLimitExceeded").error.errors[0]] } },
    { error: { code: 403, errors: [body().error.errors[0], body("forbidden", "global").error.errors[0]] } },
    { error: { code: 403, errors: [body().error.errors[0], null] } },
  ];
  const raw = ["not json", '{"error":{"code":403,"code":403,"errors":[]}}',
    '{"error":{"code":403,"errors":[{"domain":"usageLimits","reason":"rateLimitExceeded","reason":"forbidden"}]}}',
    JSON.stringify(body()).replace('"message":"' + secret + '"', '"message":NaN'),
    ...bad.map(value => JSON.stringify(value))];
  for (const payload of raw) {
    const f = fixture(() => new Response(payload, { status: 403, headers: { "Content-Type": "application/json" } }));
    await rejected(f.operations.deleteEvent("selected"), "permission");
    assert.equal(f.requests.length, 1);
  }
});

test("Calendar unusable diagnostic encodings and lengths preserve permission", async () => {
  for (const headers of [
    { "Content-Type": "text/plain" },
    { "Content-Type": "application/json", "Content-Encoding": "gzip" },
    ...["65537", "2", "-1", "1, 1"].map(size => ({ "Content-Type": "application/json", "Content-Length": size })),
  ]) {
    await rejected(fixture(() => new Response(JSON.stringify(body()), { status: 403, headers }))
      .operations.deleteEvent("selected"), "permission");
  }
  const invalid = new Uint8Array([...new TextEncoder().encode(JSON.stringify(body())), 0xff]);
  await rejected(fixture(() => new Response(invalid, {
    status: 403, headers: { "Content-Type": "application/json" },
  })).operations.deleteEvent("selected"), "permission");
});

test("Calendar diagnostics obey both the configured limit and 64 KiB cap", async () => {
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
    await rejected(f.operations.deleteEvent("selected"), "permission");
    assert.ok(pulls <= (limit === 64 ? 1 : 65));
    assert.equal(closed, true);
    assert.equal(f.requests.length, 1);
  }
});

test("Calendar diagnostic I/O failure retains permission and private prose stays absent", async () => {
  const f = fixture(() => new Response(new ReadableStream({
    pull(controller) { controller.error(new Error(secret)); },
  }), { status: 403, headers: { "Content-Type": "application/json" } }));
  await rejected(f.operations.deleteEvent("selected"), "permission");
  assert.equal(f.requests.length, 1);
});

test("Calendar diagnostic cancellation joins close and preserves uncertain writes", async () => {
  const abort = new AbortController();
  let enter, release, closed = false, settled = false;
  const entered = new Promise(resolve => { enter = resolve; });
  const cleanup = new Promise(resolve => { release = resolve; });
  const f = fixture(() => new Response(new ReadableStream({
    pull() { enter(); },
    async cancel() { await cleanup; closed = true; },
  }, { highWaterMark: 0 }), { status: 403, headers: { "Content-Type": "application/json" } }));
  const pending = f.operations.deleteEvent("selected", { signal: abort.signal });
  pending.then(() => { settled = true; }, () => { settled = true; });
  try {
    await entered;
    abort.abort(secret);
    await new Promise(resolve => setTimeout(resolve, 10));
    assert.equal(settled, false);
  } finally { release(); }
  await assert.rejects(pending, error => error instanceof CalendarCancelled
    && error.writeOutcome === "unknown" && !String(error).includes(secret));
  assert.equal(closed, true);
  assert.equal(f.requests.length, 1);
});

test("Calendar plain permission, 429 and Retry-After keep their original contract", async () => {
  for (const [status, kind] of [[403, "permission"], [429, "rate_limited"]]) {
    for (const retry of ["17", "bad-header"]) {
      const f = fixture(() => Response.json({}, { status, headers: { "Retry-After": retry } }));
      await assert.rejects(f.operations.deleteEvent("selected"), error => {
        assert.equal(error.kind, kind);
        assert.equal(error.statusCode, status);
        assert.equal(error.writeOutcome, "unknown");
        assert.equal(error.retryAfterSeconds, retry === "17" ? 17 : null);
        return true;
      });
      assert.equal(f.requests.length, 1);
    }
  }
});

test("Calendar stalled diagnostics keep the original total deadline", async () => {
  let closed = false;
  const f = fixture(() => new Response(new ReadableStream({
    cancel() { closed = true; },
  }), { status: 403, headers: { "Content-Type": "application/json" } }), { timeoutMs: 20 });
  await rejected(f.operations.deleteEvent("selected"), "timeout", "unknown", null);
  assert.equal(closed, true);
  assert.equal(f.requests.length, 1);
});
