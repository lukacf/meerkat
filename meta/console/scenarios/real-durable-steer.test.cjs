"use strict";
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { assertPeerSetupHistory } = require("./real-durable-steer.cjs");

const expected = {
  senderId: "sender-id", displayName: "console-acceptance/lead/mk--domain_cdelivery",
  body: "Review the release prerequisites. Exact marker peer-setup.", acknowledgement: "Peer setup acknowledged.",
  completion: { session_id: "session", run_id: "peer-run", interaction_id: "envelope" },
};
const incoming = () => ({ type: "comms", direction: "incoming", kind: "message",
  peer: { id: expected.senderId, display_name: expected.displayName },
  content: [{ type: "text", text: `Peer message from ${expected.displayName}:\n${expected.body}` }],
});
const history = () => ({ session_id: "session", has_more: false, message_count: 2, messages: [
  { role: "system_notice", blocks: [incoming()] },
  { role: "block_assistant", identity: { run_id: "peer-run", interaction_id: "envelope" },
    blocks: [{ block_type: "text", data: { text: expected.acknowledgement } }] },
] });

test("explicit peer history works without an incidental startup lifecycle notice", () => {
  assert.deepEqual(assertPeerSetupHistory(history(), expected), [{ id: expected.senderId,
    displayName: expected.displayName, label: "domain:delivery", count: 1, kickoffs: [] }]);
});

test("a startup kickoff notice renders as a kickoff card, apart from the explicit peer row", () => {
  for (const kind of ["lifecycle", "request"]) {
    const value = history(); value.messages.unshift({ role: "system_notice", blocks: [{
      ...incoming(), kind, intent: "mob.kickoff_started",
      content: [{ type: "text", text: "Peer lifecycle: mob.kickoff_started" }],
    }] }); value.message_count++;
    const [peer] = assertPeerSetupHistory(value, expected);
    assert.equal(peer.count, 1, `${kind}: one incoming peer row`);
    assert.deepEqual(peer.kickoffs, ["started"], `${kind}: one kickoff card`);
  }
});

test("peer setup requires exact body and canonical sender instead of any lifecycle notice", () => {
  const missing = history(); missing.messages[0].blocks[0].content[0].text = "Peer lifecycle: mob.kickoff_started";
  assert.throws(() => assertPeerSetupHistory(missing, expected), /one explicit/);
  const foreign = history(); foreign.messages[0].blocks[0].peer.id = "foreign";
  assert.throws(() => assertPeerSetupHistory(foreign, expected), /canonical sender/);
  const duplicate = history(); duplicate.messages.unshift(duplicate.messages[0]); duplicate.message_count++;
  assert.throws(() => assertPeerSetupHistory(duplicate, expected), /one explicit/);
});

test("peer setup requires its matching committed acknowledgement and complete session history", () => {
  const foreign = history(); foreign.messages[1].identity.run_id = "different-run";
  assert.throws(() => assertPeerSetupHistory(foreign, expected), /matching peer run/);
  const text = history(); text.messages[1].blocks[0].data.text = "Unrelated response";
  assert.throws(() => assertPeerSetupHistory(text, expected), /exact peer acknowledgement/);
  assert.throws(() => assertPeerSetupHistory({ ...history(), session_id: "wrong-session" }, expected), /actual recipient session/);
  assert.throws(() => assertPeerSetupHistory({ ...history(), has_more: true }, expected), /complete recipient history/);
});
