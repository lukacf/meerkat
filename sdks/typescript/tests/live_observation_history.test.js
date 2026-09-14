import { readFileSync } from "node:fs";
import { test } from "node:test";
import assert from "node:assert/strict";
import { MeerkatClient } from "../dist/client.js";
import { Mob } from "../dist/mob.js";

function pageFixture() {
  return JSON.parse(readFileSync(new URL(
    "../../../meerkat-contracts/tests/fixtures/live-observation-page-v1.json",
    import.meta.url,
  ), "utf8"));
}

test("client and mob retained-history wrappers issue only the read request", async () => {
  const client = new MeerkatClient();
  const calls = [];
  client.request = async (method, params) => {
    calls.push([method, params]);
    return pageFixture();
  };
  const result = await client.mobMemberLiveObservations("mob-history", "speaker", {
    channelId: "channel-a", cursor: "opaque", limit: 7,
  });
  assert.equal(result.page.records[0].observation.text, "heard\n\0not a Message");
  assert.equal(result.page.snapshot.coverage, "known_local_gap");
  assert.equal(result.provenance, "host_claimed");
  assert.deepEqual(calls, [["mob/member_live_observations", {
    mob_id: "mob-history", agent_identity: "speaker",
    channel_id: "channel-a", cursor: "opaque", limit: 7,
  }]]);
  calls.length = 0;
  await new Mob(client, "mob-history").memberLiveObservations("speaker");
  assert.deepEqual(calls, [["mob/member_live_observations", {
    mob_id: "mob-history", agent_identity: "speaker",
  }]]);
});

for (const limit of [0, -1, 257, 1.5, NaN, Infinity]) {
  test(`invalid limit ${limit} never sends`, async () => {
    const client = new MeerkatClient();
    client.request = async () => assert.fail("invalid query reached transport");
    await assert.rejects(
      client.mobMemberLiveObservations("mob-history", "speaker", { limit }),
      RangeError,
    );
  });
}

for (const [path, value] of [
  [["page", "encoding_profile"], "v9"],
  [["page", "owner", "kind"], "other"],
  [["page", "snapshot", "coverage"], "complete"],
  [["page", "has_more"], "false"],
  [["page", "records", 0, "observation", "direction"], "other"],
  [["page", "records", 0, "observation", "text"], null],
]) {
  test(`generated history parser rejects ${path.join(".")}`, async () => {
    const raw = pageFixture();
    let parent = raw;
    for (const key of path.slice(0, -1)) parent = parent[key];
    parent[path.at(-1)] = value;
    const client = new MeerkatClient();
    client.request = async () => raw;
    await assert.rejects(
      client.mobMemberLiveObservations("mob-history", "speaker"),
      (error) => error.code === "INVALID_RESPONSE",
    );
  });
}
