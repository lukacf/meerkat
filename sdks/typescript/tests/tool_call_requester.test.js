// The original requester on callback tool calls. A `tool/execute` request
// names the original requester of the run the call serves, typed, as
// `requester`; the runtime sets it, tool arguments never do. The SDK parses
// it strictly (a malformed value is no requester) and passes it to the
// handler as its optional third argument, so a host can refuse a
// person-scoped tool for anyone but its owner.
import assert from "node:assert/strict";
import { describe, it } from "node:test";

import { MeerkatClient } from "../dist/index.js";

const ISSUER_DOMAIN = "oidc-issuer:https://issuer.example";

const wirePrincipal = (subject) => ({
  kind: "human",
  id: subject,
  qualification: { kind: "qualified", trust_domain_id: ISSUER_DOMAIN },
});

const wireRequester = (subject, carried = false) =>
  carried ? { principal: wirePrincipal(subject), carried: true } : { principal: wirePrincipal(subject) };

const person = (subject) => ({ kind: "human", id: subject, trust_domain_id: ISSUER_DOMAIN });

/** A client whose `read_mail` tool serves only `owner`'s direct requests. */
function personScopedClient(owner, seen) {
  const client = new MeerkatClient();
  client.registerTool("read_mail", "Read mail", { type: "object" }, async (args, _key, requester) => {
    seen.push([args, requester]);
    if (
      requester === undefined ||
      requester.carried ||
      requester.principal.kind !== owner.kind ||
      requester.principal.id !== owner.id ||
      requester.principal.trust_domain_id !== owner.trust_domain_id
    ) {
      throw new Error("this mailbox serves its owner only");
    }
    return `${owner.id}'s mail`;
  });
  return client;
}

async function execute(client, args, requester) {
  const writes = [];
  client.process = { stdin: { write: (line) => writes.push(line) } };
  const params = { tool_use_id: "call-1", name: "read_mail", arguments: args };
  if (requester !== undefined) params.requester = requester;
  client.handleLine(JSON.stringify({ jsonrpc: "2.0", id: 9, method: "tool/execute", params }));
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(writes.length, 1);
  return JSON.parse(writes[0]).result;
}

describe("tool call requester", () => {
  it("parses a requester and reads a malformed one as no requester", () => {
    assert.deepEqual(MeerkatClient.parseToolCallRequester(wireRequester("alice")), {
      principal: person("alice"),
      carried: false,
    });
    assert.deepEqual(MeerkatClient.parseToolCallRequester(wireRequester("alice", true)), {
      principal: person("alice"),
      carried: true,
    });
    assert.deepEqual(
      MeerkatClient.parseToolCallRequester({ principal: { kind: "service_account", id: "svc-1" } }),
      { principal: { kind: "service_account", id: "svc-1" }, carried: false },
    );
    for (const raw of [
      undefined,
      null,
      "alice",
      [],
      {},
      { principal: null },
      { principal: { kind: "human" } },
      { principal: { kind: "human", id: "" } },
      { principal: { kind: 7, id: "alice" } },
      { principal: { ...wirePrincipal("alice"), qualification: { kind: "qualified" } } },
      { principal: { ...wirePrincipal("alice"), qualification: "issuer" } },
      { ...wireRequester("alice"), carried: "yes" },
    ]) {
      assert.equal(MeerkatClient.parseToolCallRequester(raw), undefined, JSON.stringify(raw));
    }
  });

  it("reads every requester the runtime refuses as no requester", () => {
    const qualified = { kind: "qualified", trust_domain_id: ISSUER_DOMAIN };
    const alice = wirePrincipal("alice");
    const refused = {
      "an unknown qualification kind": { ...alice, qualification: { kind: "maybe" } },
      "an unknown qualification kind with a domain": {
        ...alice,
        qualification: { kind: "maybe", trust_domain_id: ISSUER_DOMAIN },
      },
      "a blank qualification kind": { ...alice, qualification: { kind: "  " } },
      "a null qualification": { ...alice, qualification: null },
      "a trust domain on an unqualified principal": {
        ...alice,
        qualification: { kind: "unqualified", trust_domain_id: ISSUER_DOMAIN },
      },
      "a qualification field the runtime does not have": {
        ...alice,
        qualification: { ...qualified, via: "proxy" },
      },
      "a blank trust domain": { ...alice, qualification: { kind: "qualified", trust_domain_id: "  " } },
      "a trust domain with a control character": {
        ...alice,
        qualification: { kind: "qualified", trust_domain_id: `${ISSUER_DOMAIN}\u0000` },
      },
      "a blank principal kind": { ...alice, kind: "  " },
      "an unknown principal kind": { ...alice, kind: "robot" },
      "a qualified id with a control character": wirePrincipal("ali\u0007ce"),
    };
    const accepted = Object.entries(refused)
      .filter(([, principal]) => MeerkatClient.parseToolCallRequester({ principal }) !== undefined)
      .map(([name]) => name);
    assert.deepEqual(accepted, [], "the SDK reads a requester the runtime refuses");
  });

  it("lets a host refuse a person-scoped tool for another requester", async () => {
    const seen = [];
    const client = personScopedClient(person("alice"), seen);
    assert.deepEqual(await execute(client, {}, wireRequester("alice")), {
      content: "alice's mail",
      is_error: false,
    });
    for (const requester of [wireRequester("bob"), wireRequester("alice", true), undefined]) {
      const refused = await execute(client, {}, requester);
      assert.equal(refused.is_error, true, JSON.stringify(requester));
      assert.match(refused.content, /owner only/);
    }
    assert.deepEqual(
      seen.map(([, requester]) => requester),
      [
        { principal: person("alice"), carried: false },
        { principal: person("bob"), carried: false },
        { principal: person("alice"), carried: true },
        undefined,
      ],
    );
  });

  it("never takes the requester from the arguments", async () => {
    const seen = [];
    const client = personScopedClient(person("alice"), seen);
    const forged = { requester: wireRequester("alice") };
    assert.equal((await execute(client, forged, undefined)).is_error, true);
    assert.equal((await execute(client, forged, wireRequester("bob"))).is_error, true);
    assert.deepEqual(seen, [
      [forged, undefined],
      [forged, { principal: person("bob"), carried: false }],
    ]);
  });
});
