import assert from "node:assert/strict";
import test from "node:test";

import { LiveChannel } from "../dist/live.js";

test("LiveChannel forwards seedMaxChars as seed_max_chars", async () => {
  const calls = [];
  const client = {
    async liveOpen(params) {
      calls.push(params);
      return {
        channel_id: "live_seeded",
        transport: { transport: "websocket", url: "ws://x", token: "t" },
        capabilities: {},
        continuity: { mode: "degraded" },
      };
    },
  };

  const channel = LiveChannel.session(client, "session-seeded", {
    seedMaxChars: 24_000,
  });
  const result = await channel.open();

  assert.deepEqual(calls, [
    { session_id: "session-seeded", seed_max_chars: 24_000 },
  ]);
  assert.equal(channel.channelId, "live_seeded");
  assert.deepEqual(result.continuity, { mode: "degraded" });
});

test("LiveChannel omits the seed field by default", async () => {
  const calls = [];
  const client = {
    async liveOpen(params) {
      calls.push(params);
      return {
        channel_id: "live_full_seed",
        transport: { transport: "websocket", url: "ws://x", token: "t" },
        capabilities: {},
        continuity: { mode: "transcript_only" },
      };
    },
  };

  await LiveChannel.session(client, "session-full-seed").open();

  assert.deepEqual(calls, [{ session_id: "session-full-seed" }]);
});
