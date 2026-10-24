import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { resolveRequiredSmokeBinary } from "./support/required_smoke_binary.mjs";

const binaryPath = resolveRequiredSmokeBinary();

const fakeStreamBinary = fileURLToPath(
  new URL("../../test-fixtures/fake_stream_rpc.mjs", import.meta.url),
);

describe("E2E Smoke: TypeScript SDK package", () => {
  let MeerkatClient;
  let client;

  before(async () => {
    ({ MeerkatClient } = await import("../dist/index.js"));
    client = new MeerkatClient(binaryPath);
    await client.connect();
  });

  after(async () => {
    if (client) {
      await client.close();
    }
  });

  it("connects through the packaged SDK and fetches capabilities", async () => {
    const capabilities = client.capabilities;
    assert.ok(Array.isArray(capabilities), "capabilities should be an array");
    assert.ok(capabilities.length > 0, "capabilities should not be empty");
    assert.ok(
      capabilities.some((capability) => capability.id === "sessions"),
      "sessions capability should be present",
    );
  });

  it("surfaces a typed error for reading a missing session", async () => {
    await assert.rejects(
      () => client.readSession("00000000-0000-0000-0000-000000000000"),
      /Session not found|RPC error/i,
    );
  });

  it("opens and explicitly closes a live mob stream through the packaged SDK", async (t) => {
    const supportsMob = Array.isArray(client.capabilities)
      && client.capabilities.some((capability) => capability.id === "mob");
    if (!supportsMob) {
      t.skip("packaged binary does not advertise mob capability");
      return;
    }

    const mob = await client.createMob({
      definition: {
        id: "coding_swarm",
        orchestrator: { profile: "lead" },
        profiles: {
          lead: { model: "claude-opus-4-8" },
          worker: { model: "claude-sonnet-4-6" },
        },
      },
    });
    const mobId = mob.mobId;
    assert.ok(mobId, "mob_create should return mob_id");

    const sub = await client.subscribeMobEvents(mobId);
    await sub.close();

    for (let attempt = 0; attempt < 10; attempt += 1) {
      if (sub.terminalOutcome?.outcome === "explicit_close") {
        break;
      }
      await delay(20);
    }

    assert.equal(sub.terminalOutcome?.outcome, "explicit_close");
  });
});

describe("E2E Smoke: TypeScript SDK package against scripted stream RPC", () => {
  let MeerkatClient;
  let client;

  before(async () => {
    ({ MeerkatClient } = await import("../dist/index.js"));
    client = new MeerkatClient(fakeStreamBinary);
    await client.connect();
  });

  after(async () => {
    if (client) {
      await client.close();
    }
  });

  it("replays buffered standalone stream events in order and exposes remote_end", async () => {
    const sub = await client.subscribeSessionEvents("buffered-session");
    const deltas = [];
    for await (const event of sub) {
      deltas.push(event.payload.delta);
    }

    assert.deepEqual(deltas, ["hi", "there"]);
    assert.equal(sub.terminalOutcome?.outcome, "remote_end");
  });

  it("surfaces terminal_error on the packaged standalone subscription API", async () => {
    const sub = await client.subscribeSessionEvents("terminal-error-session");
    const iterator = sub[Symbol.asyncIterator]();
    const { EventSubscriptionError, MeerkatError } = await import("../dist/index.js");
    await assert.rejects(iterator.next(), (error) => {
      assert.ok(error instanceof EventSubscriptionError);
      assert.ok(error instanceof MeerkatError);
      assert.equal(error.code, "stream_queue_overflow");
      assert.equal(error.lastRpcSequence, undefined);
      assert.equal(error.lastEventSeq, undefined);
      return true;
    });
    assert.equal(sub.terminalOutcome?.outcome, "terminal_error");
    assert.equal(sub.terminalOutcome?.error?.code, "stream_queue_overflow");
  });

  it("drains late tail turn events through the packaged streaming API", async () => {
    const stream = client._startTurnStreaming(
      "late-tail-stream-session",
      "trigger late tail",
    );

    let text = "";
    for await (const event of stream) {
      if (event.type === "text_delta") {
        text += event.delta;
      }
    }

    assert.equal(text, "LATE_TAIL_PUBLIC");
    assert.equal(stream.result.text, "late tail final result");
  });
});
