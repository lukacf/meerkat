import assert from "node:assert/strict";
import test from "node:test";

import { LiveChannel } from "../dist/live.js";
import { MeerkatClient } from "../dist/client.js";
import { Mob } from "../dist/mob.js";

test("member profile selection reaches the owner request without a local grant", async () => {
  const client = new MeerkatClient();
  const calls = [];
  const refusal = new Error("owner has no public profile control");
  client.request = async (method, params) => {
    calls.push([method, params]);
    throw refusal;
  };
  await assert.rejects(new Mob(client, "mob-id").memberLiveOpen("worker", { profileId: "voice" }), (error) => error === refusal);
  assert.deepEqual(calls, [["mob/member_live_open", {
    mob_id: "mob-id", agent_identity: "worker", profile_id: "voice",
  }]]);
});

test("LiveChannel forwards public profile selection without treating it as activation", async () => {
  const calls = [];
  const refusal = new Error("public profile control is not installed");
  const client = {
    async liveOpen(params) {
      calls.push(params);
      throw refusal;
    },
  };
  const channel = LiveChannel.session(client, "session-profile", {
    profileId: "voice",
    turningMode: "continuous",
  });
  await assert.rejects(channel.open(), (error) => error === refusal);
  assert.equal(channel.channelId, undefined);
  assert.deepEqual(calls, [{
    session_id: "session-profile",
    profile_id: "voice",
    turning_mode: "continuous",
  }]);
});

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
