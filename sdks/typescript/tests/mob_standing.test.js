/**
 * `mobStanding`: where a mob stands, in the words of the wire.
 *
 * `mob/standing` answers `busy`, `finished`, `closed`, `stopped` or `unknown`
 * for a served mob, or the word for why a mob is not served. No word says
 * idle: a quiet running mob answers `unknown` with the evidence the read did
 * not have. The vocabularies may grow, so the wrapper keeps a word it does
 * not know as it was sent.
 */

import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { MeerkatClient } from "../dist/index.js";

// A quiet running mob, as a server sends it.
const QUIET = {
  mob_id: "quiet",
  standing: "unknown",
  causes: ["peer_delivery_not_traced", "handoffs_not_tracked"],
  counts: {
    roster_transitions: 0,
    runs: 0,
    parked_admission: 0,
    foreground_owners: 0,
  },
};

/** A client whose RPC transport is replaced by `answer`. */
function answeringClient(answer) {
  const client = new MeerkatClient();
  const sent = [];
  client.request = async (method, params) => {
    sent.push([method, params]);
    return answer;
  };
  return { client, sent };
}

describe("mobStanding", () => {
  it("sends the mob id and answers the standing", async () => {
    const { client, sent } = answeringClient(QUIET);
    const answered = await client.mobStanding("quiet");
    assert.deepEqual(answered, QUIET);
    assert.equal(answered.standing, "unknown");
    assert.deepEqual(sent, [["mob/standing", { mob_id: "quiet" }]]);
  });

  it("answers a mob that is not served by its word alone", async () => {
    const dormant = { mob_id: "asleep", standing: "dormant" };
    const { client } = answeringClient(dormant);
    const answered = await client.mobStanding("asleep");
    assert.deepEqual(answered, dormant);
    assert.equal("counts" in answered, false);
    assert.equal("causes" in answered, false);
  });

  it("keeps a word this SDK does not know", async () => {
    // A later server may answer a word or a cause this SDK's types do not
    // list. The call does not fail, and nothing is rewritten to a known word.
    const later = {
      mob_id: "quiet",
      standing: "a_word_from_a_later_server",
      reason: "a_reason_from_a_later_server",
      causes: ["a_cause_from_a_later_server"],
    };
    const { client } = answeringClient(later);
    const answered = await client.mobStanding("quiet");
    assert.deepEqual(answered, later);
    // Each unknown word is kept as it was sent. The SDK has no helper that
    // classifies a standing: nothing reads the word as idle or as finished,
    // and the reason and the cause are neither defaulted nor dropped.
    assert.equal(answered.standing, "a_word_from_a_later_server");
    assert.notEqual(answered.standing, "idle");
    assert.notEqual(answered.standing, "finished");
    assert.equal(answered.reason, "a_reason_from_a_later_server");
    assert.deepEqual(answered.causes, ["a_cause_from_a_later_server"]);
  });
});
