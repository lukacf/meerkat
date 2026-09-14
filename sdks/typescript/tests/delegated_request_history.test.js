import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { MeerkatClient } from "../dist/client.js";

const fixture = () => JSON.parse(readFileSync(
  new URL("../../../meerkat-contracts/tests/fixtures/delegated-request-message-v1.json", import.meta.url),
  "utf8",
));
const history = (message) => ({
  session_id: "session", message_count: 1, offset: 0, has_more: false, messages: [message],
});

test("session history preserves the exact nonhuman request slot", async () => {
  const client = new MeerkatClient();
  const message = fixture();
  client.request = async (method, params) => {
    assert.equal(method, "session/history");
    assert.deepEqual(params, { session_id: "session", offset: 0 });
    return history(message);
  };
  const result = await client.readSessionHistory("session");
  assert.equal(result.messages[0].content, message.content);
  assert.equal(result.messages[0].createdAt, message.created_at);
  assert.deepEqual(result.messages[0].transcriptRole, message.transcript_role);
  assert.deepEqual(result.messages[0].raw, message);
});

for (const [name, mutate] of [
  ["role permission", (role) => { role.permission = true; }],
  ["payload grant", (role) => { role.delegated_request.grant = true; }],
  ["promoted speech", (role) => { role.delegated_request.provenance.final_user_transcript = true; }],
  ["missing grade", (role) => { delete role.delegated_request.provenance.evidence_kind; }],
  ["invalid source", (role) => { role.delegated_request.provenance.source = "wrong shape"; }],
  ["source grant", (role) => { role.delegated_request.provenance.source.source.grant = true; }],
  ["short digest", (role) => { role.delegated_request.provenance.request_digest.pop(); }],
  ["negative digest byte", (role) => { role.delegated_request.provenance.request_digest[0] = -1; }],
  ["oversized digest byte", (role) => { role.delegated_request.provenance.request_digest[0] = 256; }],
]) {
  test(`session history rejects ${name}`, async () => {
    const client = new MeerkatClient();
    const message = fixture();
    mutate(message.transcript_role);
    client.request = async () => history(message);
    await assert.rejects(client.readSessionHistory("session"), (error) => error.code === "INVALID_RESPONSE");
  });
}
