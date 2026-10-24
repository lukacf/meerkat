/**
 * Non-streaming calls opt out of their turn's `session/event` copies.
 *
 * A call that does not stream its turn reads only the run result, so the SDK
 * sends `session_events: false` on `turn/start`, `session/create` and
 * `mob/turn_start`. It sends the field only to a server whose contract takes
 * it (0.9.5 or later): an older server refuses it on `turn/start`. A
 * streaming call never sends it.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { MeerkatClient, Session } from "../dist/index.js";

const RUN_RESULT = {
  session_id: "s1",
  text: "ok",
  turns: 1,
  tool_calls: 0,
  usage: { input_tokens: 1, output_tokens: 1 },
};

function recordingClient(contractVersion) {
  const client = new MeerkatClient();
  client._serverContractVersion = contractVersion;
  const sent = [];
  client.request = async (method, params) => {
    sent.push({ method, params });
    return structuredClone(RUN_RESULT);
  };
  client.process = {
    stdin: {
      write: (data) => {
        const { method, params } = JSON.parse(data);
        sent.push({ method, params });
      },
    },
  };
  client.registerRequest = async () => structuredClone(RUN_RESULT);
  return { client, sent };
}

function session(client) {
  return new Session(client, {
    sessionId: "s1",
    text: "ready",
    turns: 0,
    toolCalls: 0,
    usage: { inputTokens: 0, outputTokens: 0 },
  });
}

describe("turn session/event opt-out", () => {
  for (const [contractVersion, optsOut] of [
    ["0.9.5", true],
    ["0.9.10", true],
    ["0.10.0", true],
    ["0.9.4", false],
    ["0.9.x", false],
    ["", false],
  ]) {
    it(`a non-streaming turn against contract "${contractVersion}" ${optsOut ? "opts out" : "sends no field"}`, async () => {
      const { client, sent } = recordingClient(contractVersion);

      await session(client).turn("next");

      assert.equal(sent.length, 1);
      assert.equal(sent[0].method, "turn/start");
      if (optsOut) {
        assert.equal(sent[0].params.session_events, false);
      } else {
        assert.equal("session_events" in sent[0].params, false);
      }
    });
  }

  it("createSession opts out of its first turn's copies", async () => {
    const { client, sent } = recordingClient("0.9.5");

    await client.createSession("hello");

    assert.equal(sent[0].method, "session/create");
    assert.equal(sent[0].params.session_events, false);
  });

  it("mobTurnStart opts out of the member turn's copies", async () => {
    const { client, sent } = recordingClient("0.9.5");

    await client.mobTurnStart("mob-1", "agent-a", "hello");

    assert.equal(sent[0].method, "mob/turn_start");
    assert.equal(sent[0].params.session_events, false);
  });

  it("a streaming turn keeps its session events", () => {
    const { client, sent } = recordingClient("0.9.5");

    session(client).stream("next");

    assert.equal(sent.length, 1);
    assert.equal(sent[0].method, "turn/start");
    assert.equal("session_events" in sent[0].params, false);
  });
});
