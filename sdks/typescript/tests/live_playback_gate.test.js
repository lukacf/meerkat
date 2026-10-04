import assert from "node:assert/strict";
import test from "node:test";

import {
  applyLiveAssistantPlaybackHint,
  LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN,
  LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S,
  LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN,
} from "../dist/index.js";

function recordingGain() {
  const calls = [];
  return {
    calls,
    gain: {
      cancelScheduledValues(time) {
        calls.push(["cancel", time]);
      },
      setTargetAtTime(target, start, timeConstant) {
        calls.push(["target", target, start, timeConstant]);
      },
    },
  };
}

test("a duck hint ramps assistant playback to silence on the audio clock", () => {
  const node = recordingGain();
  applyLiveAssistantPlaybackHint(node, "duck", 12.5);
  assert.deepEqual(node.calls, [
    ["cancel", 12.5],
    ["target", LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN, 12.5, LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S],
  ]);
  assert.equal(LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN, 0);
});

test("a restore hint returns assistant playback to unity gain", () => {
  const node = recordingGain();
  applyLiveAssistantPlaybackHint(node, "restore", 3);
  assert.deepEqual(node.calls.at(-1), [
    "target",
    LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN,
    3,
    LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S,
  ]);
  assert.equal(LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN, 1);
});

test("a gain stage without cancelScheduledValues still takes the hint", () => {
  const calls = [];
  applyLiveAssistantPlaybackHint(
    { gain: { setTargetAtTime: (...args) => calls.push(args) } },
    "duck",
    0,
  );
  assert.equal(calls.length, 1);
});

// The Turbo S browser peer applies this exact gate policy to its assistant
// audio; it cannot import this package, so its constants are checked here.
test("the e2e browser peer's playback gate matches the SDK gate", async () => {
  const { readFile } = await import("node:fs/promises");
  const harness = await readFile(
    new URL("../../../tests/live_smoke/browser/harness/gpt-live-peer-e2e.mjs", import.meta.url),
    "utf8",
  );
  const match = harness.match(
    /const ASSISTANT_PLAYBACK_GATE = \{ duckedGain: ([0-9.]+), unityGain: ([0-9.]+), timeConstantS: ([0-9.]+) \};/,
  );
  assert.ok(match, "the peer declares ASSISTANT_PLAYBACK_GATE");
  assert.equal(Number(match[1]), LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN);
  assert.equal(Number(match[2]), LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN);
  assert.equal(Number(match[3]), LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S);
});

test("live/* notifications reach onLiveNotification listeners, typed, and never a session queue", async () => {
  const { MeerkatClient } = await import("../dist/client.js");
  const client = new MeerkatClient();
  const seen = [];
  const unsubscribe = client.onLiveNotification((notification) => seen.push(notification));
  const notify = (method, params) =>
    client.handleLine(JSON.stringify({ jsonrpc: "2.0", method, params }));

  notify("live/assistant_playback_hint", { channel_id: "ch-1", hint: "duck" });
  notify("live/assistant_playback_hint", { channel_id: "ch-1", hint: "restore" });
  notify("live/media_health_requested", { channel_id: "ch-1", output_id: "out-1" });
  notify("live/assistant_output_available", {
    channel_id: "ch-1",
    output_id: "out-2",
    content_index: 0,
  });
  // Unknown live methods and malformed payloads are ignored, not misread.
  notify("live/some_future_notification", { channel_id: "ch-1" });
  notify("live/assistant_playback_hint", { channel_id: "ch-1", hint: "louder" });
  notify("live/assistant_playback_hint", { hint: "duck" });

  assert.deepEqual(seen, [
    { method: "live/assistant_playback_hint", params: { channel_id: "ch-1", hint: "duck" } },
    { method: "live/assistant_playback_hint", params: { channel_id: "ch-1", hint: "restore" } },
    { method: "live/media_health_requested", params: { channel_id: "ch-1", output_id: "out-1" } },
    {
      method: "live/assistant_output_available",
      params: { channel_id: "ch-1", output_id: "out-2", content_index: 0 },
    },
  ]);
  assert.equal(client.eventQueues.size, 0, "no session queue was touched");
  assert.equal(client.unmatchedStreamBuffer.size, 0, "nothing buffered as a session event");

  unsubscribe();
  notify("live/assistant_playback_hint", { channel_id: "ch-1", hint: "duck" });
  assert.equal(seen.length, 4, "an unsubscribed listener receives nothing");
});

test("a throwing live listener does not stop the next one", async () => {
  const { MeerkatClient } = await import("../dist/client.js");
  const client = new MeerkatClient();
  const seen = [];
  client.onLiveNotification(() => {
    throw new Error("listener fault");
  });
  client.onLiveNotification((notification) => seen.push(notification.params.hint));
  client.handleLine(
    JSON.stringify({
      jsonrpc: "2.0",
      method: "live/assistant_playback_hint",
      params: { channel_id: "ch-1", hint: "duck" },
    }),
  );
  assert.deepEqual(seen, ["duck"]);
});
