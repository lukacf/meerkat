import assert from "node:assert/strict";
import { test } from "vitest";

import {
  normalizeTopologyLineage,
  normalizeTopologyMemoryScopes,
  normalizeTopologyRoutes,
  topologyRoutesAccessFromError,
} from "./topology-relations";

test("routes keep their channel and recipient, mark roster routes, and count matched deliveries", () => {
  const routes = normalizeTopologyRoutes(
    {
      routes: [
        { route_key: "billing-escalations", channel: "email", recipient: "finance-lead@example.com", sink: "email", target_module: "delivery" },
        { route_key: "mob.member.billing", channel: "notification", recipient: "billing", sink: "mob_member", target_module: "delivery" },
        { route_key: "support-inbox", channel: "email", recipient: "support", sink: "mob_member", target_module: "delivery" },
        { route_key: "", channel: "email", recipient: "nobody" },
        { route_key: "no-recipient", channel: "email" },
      ],
    },
    {
      deliveries: [
        { route_id: "route-000001", recipient: "finance-lead@example.com", sink: "email", target_module: "delivery", status: "sent", final_attempt_ms: 20 },
        { route_id: "route-000002", recipient: "finance-lead@example.com", sink: "email", target_module: "delivery", status: "failed", final_attempt_ms: 30 },
        { route_id: "route-000003", recipient: "finance-lead@example.com", sink: "sms", target_module: "delivery", status: "failed", final_attempt_ms: 40 },
      ],
    },
  );
  assert.deepEqual(routes, [
    {
      key: "billing-escalations",
      channel: "email",
      recipient: "finance-lead@example.com",
      sink: "email",
      module: "delivery",
      managed: false,
      deliveries: { sent: 1, failed: 1, lastAtMs: 30 },
    },
    {
      key: "mob.member.billing",
      channel: "notification",
      recipient: "billing",
      sink: "mob_member",
      module: "delivery",
      managed: true,
      deliveries: null,
    },
    {
      key: "support-inbox",
      channel: "email",
      recipient: "support",
      sink: "mob_member",
      module: "delivery",
      managed: false,
      deliveries: null,
    },
  ]);
  assert.deepEqual(normalizeTopologyRoutes(null), []);
  assert.deepEqual(normalizeTopologyRoutes({ routes: "nope" }), []);
});

test("lineage reads forks, spawns and side chats from member_spawned events", () => {
  const spawned = (cursor: number, identity: string, data: Record<string, unknown>) => ({
    cursor,
    kind: "member_spawned",
    agent_identity: identity,
    timestamp_ms: cursor * 10,
    data: { type: "member_spawned", agent_identity: identity, ...data },
  });
  const lineage = normalizeTopologyLineage({
    events: [
      { cursor: 1, kind: "mob_created", data: { type: "mob_created" } },
      spawned(2, "billing", { creation: { creation_id: "c-billing", provenance: { kind: "unproven" } } }),
      spawned(3, "support", { creation: { creation_id: "c-support" } }),
      // A side chat: a person's fork of billing's conversation.
      spawned(4, "billing-refund-review", {
        creation: { creation_id: "c-review", provenance: { kind: "fork", source_creation_id: "c-billing" } },
        fork_source: { source_member: { member: "billing", mob_id: "ops" } },
        labels: { display_name: "Refund review", side_chat_of: "billing" },
      }),
      // A fork known only by its source creation.
      spawned(5, "draft", { creation: { creation_id: "c-draft", provenance: { kind: "fork", source_creation_id: "c-support" } } }),
      spawned(6, "ticket-4411", { spawned_by: "support" }),
      // A respawn names no parent and keeps the lineage.
      spawned(7, "ticket-4411", { generation: 1 }),
      spawned(8, "writer", { labels: { delegate_host_identity: "billing" } }),
      { cursor: 9, kind: "members_wired", data: { type: "members_wired", a: "billing", b: "support" } },
    ],
    next_after_seq: 9,
  });
  assert.deepEqual(lineage, [
    { parent: "billing", child: "billing-refund-review", kind: "side_chat", atMs: 40 },
    { parent: "support", child: "draft", kind: "fork", atMs: 50 },
    { parent: "support", child: "ticket-4411", kind: "spawn", atMs: 60 },
    { parent: "billing", child: "writer", kind: "delegate", atMs: 80 },
  ]);
  assert.deepEqual(normalizeTopologyLineage({}), []);
});

test("memory scopes are the agents' own identity scopes with their active records", () => {
  assert.deepEqual(
    normalizeTopologyMemoryScopes({
      scopes: [
        { scope_kind: "identity", scope_key: "billing", active: 12, quarantined: 1 },
        { scope_kind: "realm", scope_key: "default", active: 40 },
        { scope_kind: "identity", scope_key: "", active: 3 },
        { scope_kind: "identity", scope_key: "dispatch" },
      ],
    }),
    [
      { identity: "billing", records: 12, quarantined: 1 },
      { identity: "dispatch", records: 0, quarantined: 0 },
    ],
  );
});

test("a refused routes read means no routing access; anything else means unavailable", () => {
  const rpc = (code: number, kind?: string) => Object.assign(new Error("refused"), {
    rpcError: { code, message: "refused", data: kind ? { kind } : undefined },
  });
  assert.equal(topologyRoutesAccessFromError(rpc(-32030, "access_denied")), "denied");
  assert.equal(topologyRoutesAccessFromError(rpc(-32000, "access_denied")), "denied");
  assert.equal(topologyRoutesAccessFromError(rpc(-32001)), "denied");
  assert.equal(topologyRoutesAccessFromError(rpc(-32601)), "unavailable");
  assert.equal(topologyRoutesAccessFromError(new Error("network down")), "unavailable");
  assert.equal(topologyRoutesAccessFromError(null), "unavailable");
});
