// Callback tool dedupe key delivery. A `tool/execute` request that an
// agent-loop round dispatched carries the call's `dedupe_key`; the SDK parses
// it tolerantly and passes it to the handler as its optional second argument.
// Unrun until the candidate's validation.
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { MeerkatClient } from "../dist/index.js";

const wireKey = () => ({
  run_id: "0b9f3c55-6b8e-4f5c-9a51-2a7d1c4e8f10",
  round: 2,
  call_id: "call-1",
  call_digest: "a".repeat(64),
});

describe("tool call dedupe key", () => {
  it("parses a complete key and ignores unknown fields", () => {
    assert.deepEqual(MeerkatClient.parseToolCallDedupeKey(wireKey()), wireKey());
    assert.deepEqual(
      MeerkatClient.parseToolCallDedupeKey({ ...wireKey(), later_field: true }),
      wireKey(),
    );
  });

  it("reads an absent or malformed key as no key", () => {
    const { call_digest: _omitted, ...withoutDigest } = wireKey();
    for (const raw of [
      undefined,
      null,
      "call-1",
      [],
      {},
      withoutDigest,
      { ...wireKey(), round: -1 },
      { ...wireKey(), round: 1.5 },
      { ...wireKey(), round: "2" },
      { ...wireKey(), run_id: 7 },
    ]) {
      assert.equal(MeerkatClient.parseToolCallDedupeKey(raw), undefined, JSON.stringify(raw));
    }
  });

  for (const withKey of [true, false]) {
    it(`passes a tool/execute request's key to the handler (${withKey ? "with" : "without"} a key)`, async () => {
      const client = new MeerkatClient();
      const seen = [];
      // Registered before a process is attached, so no tools/register
      // round-trip is written; only the callback response is captured.
      client.registerTool("search", "Search", { type: "object" }, async (args, dedupeKey) => {
        seen.push(dedupeKey);
        return `searched ${args.q}`;
      });
      const writes = [];
      client.process = { stdin: { write: (line) => writes.push(line) } };

      const params = { tool_use_id: "call-1", name: "search", arguments: { q: "meerkat" } };
      if (withKey) params.dedupe_key = wireKey();
      client.handleLine(JSON.stringify({ jsonrpc: "2.0", id: 7, method: "tool/execute", params }));
      await new Promise((resolve) => setImmediate(resolve));

      assert.deepEqual(seen, [withKey ? wireKey() : undefined]);
      assert.equal(writes.length, 1);
      const response = JSON.parse(writes[0]);
      assert.equal(response.id, 7);
      assert.deepEqual(response.result, { content: "searched meerkat", is_error: false });
    });
  }
});
