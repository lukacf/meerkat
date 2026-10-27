import test from "node:test";
import assert from "node:assert/strict";
import { GmailOperations, GmailError } from "@rkat/toolkit/connectors/gmail";
import { GoogleCalendarOperations, CalendarError } from "@rkat/toolkit/connectors/google-calendar";
import { SlackOperations, SlackError } from "@rkat/toolkit/connectors/slack";

// Exercise the public injected Fetch contract. A host can supply a Response
// independently of the network engine, which must not become receipt authority.
const fixtures = [
  {
    name: "Gmail read", method: "GET", Error: GmailError, outcome: "not_applicable",
    result: { id: "message1", threadId: "thread1", snippet: "kök" },
    build: (fetch) => new GmailOperations({ baseUrl: "https://gmail.example.test/", userId: "me", fetch }),
    invoke: (api) => api.getMessage("message1"),
  },
  {
    name: "Gmail send", method: "POST", Error: GmailError, outcome: "unknown",
    result: { id: "message1", threadId: "thread1", snippet: "kök" },
    build: (fetch) => new GmailOperations({ baseUrl: "https://gmail.example.test/", userId: "me", fetch }),
    invoke: (api) => api.sendRaw(new TextEncoder().encode("Subject: fixture\r\n\r\nbody")),
  },
  {
    name: "Calendar read", method: "GET", Error: CalendarError, outcome: "not_applicable",
    result: { id: "event1", etag: '"revision1"', updated: "2026-10-02T00:00:00Z", summary: "kök" },
    build: (fetch) => new GoogleCalendarOperations({ baseUrl: "https://calendar.example.test/", calendarId: "primary", fetch }),
    invoke: (api) => api.getEvent("event1"),
  },
  {
    name: "Calendar insert", method: "POST", Error: CalendarError, outcome: "unknown",
    result: { id: "event1", etag: '"revision1"', updated: "2026-10-02T00:00:00Z", summary: "kök" },
    build: (fetch) => new GoogleCalendarOperations({ baseUrl: "https://calendar.example.test/", calendarId: "primary", fetch }),
    invoke: (api) => api.insertEvent({ summary: "fixture" }),
  },
  {
    name: "Slack read", method: "GET", Error: SlackError,
    result: { ok: true, messages: [{ ts: "123.000001", text: "kök" }] },
    build: (fetch) => new SlackOperations({ baseUrl: "https://slack.example.test/", fetch }),
    invoke: (api) => api.historyPage("C1"),
  },
];

for (const fixture of fixtures) {
  for (const delta of [-1, 1, 0, null]) {
    test(`${fixture.name}: declared byte length ${delta === null ? "absent" : delta}`, async () => {
      const bytes = new TextEncoder().encode(JSON.stringify(fixture.result));
      let calls = 0;
      let response;
      const api = fixture.build(async (request) => {
        calls++;
        assert.equal(request.method, fixture.method);
        assert.equal(request.redirect, "manual");
        assert.equal(request.headers.get("accept-encoding"), "identity");
        const headers = { "Content-Type": "application/json" };
        if (delta !== null) headers["Content-Length"] = String(bytes.length + delta);
        // Split the UTF-8 encoding of the multibyte character across chunks.
        const split = bytes.indexOf(0xc3) + 1;
        assert.ok(split > 0);
        response = new Response(new ReadableStream({ start(controller) {
          controller.enqueue(bytes.subarray(0, split));
          controller.enqueue(bytes.subarray(split));
          controller.close();
        } }), { status: 200, headers });
        return response;
      });
      const result = fixture.invoke(api);
      if (delta === -1 || delta === 1) {
        await assert.rejects(result, (error) => {
          assert.ok(error instanceof fixture.Error);
          assert.equal(error.kind, "invalid_response");
          if (fixture.outcome !== undefined) assert.equal(error.writeOutcome, fixture.outcome);
          assert.equal(error.statusCode, 200);
          assert.equal(error.cause, undefined);
          return true;
        });
      } else assert.deepEqual(await result, fixture.result);
      assert.equal(calls, 1, "validation never replays the operation");
      assert.equal(response.bodyUsed, true);
      assert.equal(response.body.locked, false);
    });
  }
}
