#!/usr/bin/env node
// Deterministic smoke for the access control pack. Drives the example server
// directly over HTTP (no browser, no API key) and asserts that each persona's
// experience, send affordances, and SSE gating match the seeded scenario.
//
// Usage: node smoke.mjs http://127.0.0.1:7300
import assert from "node:assert/strict";
import { mintToken } from "./tokens.mjs";

const baseUrl = (process.argv[2] || "http://127.0.0.1:7300").replace(/\/$/, "");

function authHeaders(email) {
  return email ? { authorization: `Bearer ${mintToken(email)}` } : {};
}

async function experience(email) {
  const res = await fetch(`${baseUrl}/console/experience`, { headers: authHeaders(email) });
  assert.equal(res.status, 200, `experience status for ${email ?? "anonymous"}`);
  return res.json();
}

function sidebarIdentities(exp) {
  return (exp.agent_sidebar?.live_snapshot?.agents ?? [])
    .map((agent) => agent.identity ?? agent.agent_id)
    .sort();
}

function affordance(exp, identity, key) {
  const agent = (exp.agent_sidebar?.live_snapshot?.agents ?? []).find(
    (candidate) => (candidate.identity ?? candidate.agent_id) === identity,
  );
  return agent?.affordances?.[key] ?? false;
}

/** SSE status: fetch resolves once headers arrive; abort before consuming the body. */
async function sseStatus(path, email) {
  const controller = new AbortController();
  try {
    const res = await fetch(`${baseUrl}${path}`, {
      headers: authHeaders(email),
      signal: controller.signal,
    });
    return res.status;
  } finally {
    controller.abort();
  }
}

async function rpc(method, params, email) {
  const res = await fetch(`${baseUrl}/console/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json", ...authHeaders(email) },
    body: JSON.stringify({ jsonrpc: "2.0", id: "smoke", method, params }),
  });
  return res.json();
}

async function main() {
  // --- Experience filtering per persona ---------------------------------
  const anon = await experience(null);
  assert.deepEqual(sidebarIdentities(anon), [], "anonymous sees no agents");
  assert.equal(anon.access.enabled, true, "access control is enforcing");
  assert.equal(anon.access.can_administer, false, "anonymous cannot administer");

  const alice = await experience("alice@example.test");
  assert.deepEqual(
    sidebarIdentities(alice),
    ["ops-lead", "scout-1", "scout-2"],
    "alice (ops) views every agent",
  );
  assert.equal(affordance(alice, "ops-lead", "can_send_message"), true, "alice may send ops-lead");
  assert.equal(
    affordance(alice, "scout-1", "can_send_message"),
    false,
    "alice may not send scout-1",
  );
  assert.equal(alice.access.can_administer, false, "alice is not an admin");
  assert.deepEqual(alice.access.groups, ["ops"], "alice is in the ops group");

  const bob = await experience("bob@example.test");
  assert.deepEqual(sidebarIdentities(bob), ["scout-1"], "bob sees only org=payments (scout-1)");
  assert.equal(affordance(bob, "scout-1", "can_send_message"), true, "bob may send scout-1");

  const root = await experience("root@example.test");
  assert.deepEqual(
    sidebarIdentities(root),
    ["ops-lead", "scout-1", "scout-2"],
    "root sees every agent",
  );
  assert.equal(root.access.can_administer, true, "root can administer");

  // --- RPC enforcement ---------------------------------------------------
  const aliceDenied = await rpc(
    "meta/console/send",
    { identity: "scout-1", content: "hi", origin: "smoke", idempotency_key: "s1" },
    "alice@example.test",
  );
  assert.equal(aliceDenied.error?.code, -32030, "alice send to scout-1 is access-denied");
  assert.equal(aliceDenied.error?.data?.kind, "access_denied");

  const aliceAllowed = await rpc(
    "meta/console/send",
    { identity: "ops-lead", content: "hi", origin: "smoke", idempotency_key: "s2" },
    "alice@example.test",
  );
  assert.notEqual(aliceAllowed.error?.code, -32030, "alice send to ops-lead is allowed");

  const anonAdmin = await rpc("meta/access/get", {}, null);
  assert.equal(anonAdmin.error?.code, -32030, "anonymous cannot read access config");

  // --- SSE gating --------------------------------------------------------
  assert.equal(await sseStatus("/agents/scout-1/events", "bob@example.test"), 200, "bob streams scout-1");
  assert.equal(await sseStatus("/agents/scout-2/events", "bob@example.test"), 403, "bob denied scout-2");
  assert.equal(await sseStatus("/mob/events", "bob@example.test"), 403, "bob lacks mob.observe");
  assert.equal(await sseStatus("/agents/scout-2/events", "root@example.test"), 200, "root streams any agent");

  console.log("[access-control-pack] smoke OK");
}

main().catch((err) => {
  console.error("[access-control-pack] smoke FAILED:", err.message);
  process.exit(1);
});
