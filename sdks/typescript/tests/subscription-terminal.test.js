import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { EventSubscriptionError, MeerkatClient, MeerkatError } from "../dist/index.js";

// Shared with the Python SDK: no native build or provider is needed.
const fixture = fileURLToPath(new URL("../../python/tests/fixtures/fake_stream_rpc.mjs", import.meta.url));

async function openSubscription(client, scope, scenario) {
  if (scope === "session") return client.subscribeSessionEvents(scenario);
  if (scope === "member") return client.subscribeMobMemberEvents(scenario, "synthetic-agent");
  return client.subscribeMobEvents(scenario);
}

describe("standalone subscription terminal errors", () => {
  it("retains events and terminal when EOF follows the open response before registration", async () => {
    const client = new MeerkatClient(fixture);
    const terminal = { stream_id: "stream-1", ended: true, outcome: "terminal_error",
      error: { code: "future_stream_code", message: "retained before EOF" } };
    client.process = { stdin: { write(line) {
      const request = JSON.parse(line);
      assert.equal(request.method, "session/stream_open");
      client.handleLine(JSON.stringify({ jsonrpc: "2.0", id: request.id,
        result: { stream_id: "stream-1" } }));
      client.handleLine(JSON.stringify({ jsonrpc: "2.0", method: "session/stream_event", params: {
        stream_id: "stream-1", sequence: 19, event: {
          event_id: "00000000-0000-4000-8000-000000000020", source: { type: "callback" },
          seq: 41, timestamp_ms: 1, payload: { type: "text_delta", delta: "queued before open resumes" },
        },
      } }));
      client.handleLine(JSON.stringify({ jsonrpc: "2.0", method: "session/stream_end", params: terminal }));
      // The response promise is resolved, but the opener cannot resume until
      // this synchronous delivery and transport queue cleanup have finished.
      client.closeQueues();
    } } };
    const sub = await client.subscribeSessionEvents("synthetic-session");
    client.process = null;
    const iterator = sub[Symbol.asyncIterator]();
    assert.equal((await iterator.next()).value.payload.delta, "queued before open resumes");
    await assert.rejects(iterator.next(), (error) => {
      assert.ok(error instanceof EventSubscriptionError);
      assert.deepEqual(error.terminalOutcome, terminal);
      assert.deepEqual([error.lastRpcSequence, error.lastEventSeq], [19, 41]);
      return true;
    });
    assert.deepEqual(sub.terminalOutcome, terminal);
  });

  it("retains a received terminal through dispatcher cleanup before iteration", async () => {
    const client = new MeerkatClient(fixture);
    client.request = async () => ({ stream_id: "stream-1" });
    const sub = await client.subscribeSessionEvents("synthetic-session");
    const terminal = { stream_id: "stream-1", ended: true, outcome: "terminal_error",
      error: { code: "future_stream_code", message: "exact terminal detail" } };
    client.handleLine(JSON.stringify({ jsonrpc: "2.0", method: "session/stream_event", params: {
      stream_id: "stream-1", sequence: 19, event: {
        event_id: "00000000-0000-4000-8000-000000000020", source: { type: "callback" },
        seq: 41, timestamp_ms: 1, payload: { type: "text_delta", delta: "before cleanup" },
      },
    } }));
    client.handleLine(JSON.stringify({ jsonrpc: "2.0", method: "session/stream_end", params: terminal }));
    client.closeQueues();
    assert.equal(client.streamTerminalOutcomes.size, 0);
    const iterator = sub[Symbol.asyncIterator]();
    assert.equal((await iterator.next()).value.payload.delta, "before cleanup");
    await assert.rejects(iterator.next(), (error) => {
      assert.ok(error instanceof EventSubscriptionError);
      assert.deepEqual(error.terminalOutcome, terminal);
      assert.deepEqual([error.lastRpcSequence, error.lastEventSeq], [19, 41]);
      return true;
    });
    assert.deepEqual(sub.terminalOutcome, terminal);
  });

  for (const raw of ["not an error record", {}, { code: 7, message: false }]) {
    it(`retains malformed terminal data ${JSON.stringify(raw)}`, () => {
      const terminal = { stream_id: "stream-1", outcome: "future_outcome", error: raw };
      const error = new EventSubscriptionError("stream-1", terminal, undefined, undefined);
      assert.ok(error instanceof MeerkatError);
      assert.equal(error.code, "INVALID_RESPONSE");
      assert.deepEqual(error.terminalOutcome, terminal);
    });
  }

  for (const scope of ["session", "mob", "member"]) {
    it(`${scope} preserves the last canonical sequence across a gap marker and terminal error`, async () => {
      const client = new MeerkatClient(fixture);
      await client.connect();
      try {
        const sub = await openSubscription(client, scope, "queued-gap-terminal-error");
        const iterator = sub[Symbol.asyncIterator]();
        const first = (await iterator.next()).value;
        assert.equal((scope === "mob" ? first.envelope : first).seq, 41);
        assert.deepEqual([sub.lastRpcSequence, sub.lastEventSeq], [7, 41]);
        const marker = (await iterator.next()).value;
        const markerEnvelope = scope === "mob" ? marker.envelope : marker;
        assert.equal(markerEnvelope.seq, 0);
        assert.deepEqual(markerEnvelope.payload, {
          type: "stream_truncated", reason: { kind: "stream_lagged", dropped: 1 },
        });
        assert.deepEqual([sub.lastRpcSequence, sub.lastEventSeq], [9, 41]);
        await assert.rejects(iterator.next(), (error) => {
          assert.ok(error instanceof EventSubscriptionError);
          assert.equal(error.code, "stream_queue_overflow");
          assert.deepEqual([error.lastRpcSequence, error.lastEventSeq], [9, 41]);
          assert.equal(error.details.last_rpc_sequence, 9);
          assert.equal(error.details.last_event_seq, 41);
          assert.deepEqual(error.terminalOutcome, sub.terminalOutcome);
          return true;
        });
      } finally {
        await client.close();
      }
    });

    for (const [scenario, code, message, outcome] of [
      ["queued-terminal-error", "stream_queue_overflow", "synthetic stream overflow", "terminal_error"],
      ["queued-unknown-error", "future_stream_code", "future detail: preserve exactly", "future_terminal_outcome"],
      ["live-terminal-error", "stream_queue_overflow", "synthetic stream overflow", "terminal_error"],
    ]) {
      it(`${scope} drains ${scenario} before raising with exact delivered sequences`, async () => {
        const client = new MeerkatClient(fixture);
        await client.connect();
        try {
          const sub = await openSubscription(client, scope, scenario);
          assert.equal(sub.lastRpcSequence, undefined);
          assert.equal(sub.lastEventSeq, undefined);
          const iterator = sub[Symbol.asyncIterator]();
          const first = await iterator.next();
          assert.equal((scope === "mob" ? first.value.envelope : first.value).payload.delta, "first");
          assert.deepEqual([sub.lastRpcSequence, sub.lastEventSeq], [7, 41]);
          const second = await iterator.next();
          assert.equal((scope === "mob" ? second.value.envelope : second.value).payload.delta, "second");
          assert.deepEqual([sub.lastRpcSequence, sub.lastEventSeq], [8, 42]);
          await assert.rejects(iterator.next(), (error) => {
            assert.ok(error instanceof EventSubscriptionError);
            assert.ok(error instanceof MeerkatError);
            assert.equal(error.code, code);
            assert.equal(error.message, message);
            assert.equal(error.streamId, sub.id);
            assert.deepEqual([error.lastRpcSequence, error.lastEventSeq], [8, 42]);
            assert.deepEqual(error.terminalOutcome, sub.terminalOutcome);
            assert.equal(error.terminalOutcome.outcome, outcome);
            assert.deepEqual(error.terminalOutcome.error, { code, message });
            assert.deepEqual(error.details, { stream_id: sub.id, terminal_outcome: sub.terminalOutcome,
              last_rpc_sequence: 8, last_event_seq: 42 });
            return true;
          });
          assert.equal((await iterator.next()).done, true);
        } finally {
          await client.close();
        }
      });
    }

    for (const outcome of ["remote_end", "explicit_close"]) {
      it(`${scope} ${outcome} without error ends normally`, async () => {
        const client = new MeerkatClient(fixture);
        await client.connect();
        try {
          const sub = await openSubscription(client, scope, `queued-${outcome.replaceAll("_", "-")}`);
          const events = [];
          for await (const event of sub) events.push(event);
          assert.equal(events.length, 2);
          assert.equal(sub.terminalOutcome.outcome, outcome);
          assert.equal(sub.terminalOutcome.error ?? null, null);
          assert.deepEqual([sub.lastRpcSequence, sub.lastEventSeq], [8, 42]);
        } finally {
          await client.close();
        }
      });
    }
  }
});
