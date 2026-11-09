import assert from "node:assert/strict";
import { describe, test } from "vitest";

import type { ConsoleAgent } from "../types";
import {
  backfillTopologyLedger,
  normalizeTopologyLineage,
  normalizeTopologyMemoryScopes,
  normalizeTopologyMemoryStore,
  normalizeTopologyRoutes,
  readTopologyRelations,
  topologyAgentStatus,
  topologyGrouping,
  topologyRoutesAccessFromError,
  topologyWiringRoles,
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

const agent = (extra: Partial<ConsoleAgent>): ConsoleAgent => ({
  identity: "billing",
  agent_id: "billing",
  member_id: "billing",
  label: "Billing",
  kind: "mob_agent",
  state: "active",
  ...extra,
});

test("the team's store counts every scope's active records", () => {
  assert.deepEqual(
    normalizeTopologyMemoryStore({ scopes: [{ scope_kind: "identity", active: 12 }, { scope_kind: "realm", active: 40 }, { active: "x" }] }),
    { id: "team", label: "Team memory", records: 52 },
  );
});

test("lanes read the sidebar's words: its buckets, or the configured grouping", () => {
  const buckets = topologyGrouping(null);
  assert.equal(buckets.sectionOf(agent({ group: "Coordinators" })), "Coordinators");
  assert.equal(buckets.sectionOf(agent({ group: "Specialists" })), "Domains");
  assert.equal(buckets.sectionOf(agent({ role: "monitor" })), "Internal");
  assert.deepEqual(buckets.order, ["Personal", "Coordinators", "Domains", "Internal", "Other"]);
  const ordered = topologyGrouping({ section_order: ["Internal"] });
  assert.deepEqual(ordered.order?.slice(0, 2), ["Internal", "Personal"]);
  const configured = topologyGrouping({ group_by: ["labels.desk", "role"], fallback_group: "Unfiled", section_order: ["Front", "Back"] });
  assert.equal(configured.sectionOf(agent({ labels: { desk: "Front" } })), "Front");
  assert.equal(configured.sectionOf(agent({ role: "triage" })), "triage");
  assert.equal(configured.sectionOf(agent({})), "Unfiled");
  assert.deepEqual(configured.order, ["Front", "Back"]);
});

test("the map's status rule: working from the owner's facts, health as a badge, busy never unobserved", () => {
  assert.deepEqual(topologyAgentStatus(agent({ response_phase: "generating" })), { working: true, health: null, healthWord: null, retired: false });
  assert.equal(topologyAgentStatus(agent({}), true).working, true);
  assert.equal(topologyAgentStatus(agent({ response_phase: null }), true).working, false);
  const progress = (health: string) => ({ health, run_state: "idle", in_flight_work: 0, last_progress_at_ms: 0, last_progress_event: "unchanged" });
  assert.deepEqual(topologyAgentStatus(agent({ progress: progress("degraded") })), { working: false, health: "warn", healthWord: "Degraded", retired: false });
  assert.deepEqual(topologyAgentStatus(agent({ progress: progress("wedged") })), { working: false, health: "crit", healthWord: "Stalled", retired: false });
  assert.equal(topologyAgentStatus(agent({ state: "retired" })).retired, true);
});

describe("the topology's relation reads", () => {
  const methods = {
    listRoutingRoutes: "meta/routing/routes/list",
    listDeliveryHistory: "meta/delivery/history",
    topologyMobEvents: "meta/mob_events/query",
    getMemoryOverview: "meta/memory/panel/overview",
  } as Record<string, string>;
  const methodOf = (command: string) => methods[command] || `meta/${command}`;
  const answers: Record<string, unknown> = {
    listRoutingRoutes: { routes: [{ route_key: "mob.member.billing", channel: "notification", recipient: "billing", sink: "mob_member" }] },
    listDeliveryHistory: { deliveries: [] },
    topologyMobEvents: { events: [{ kind: "member_spawned", data: { type: "member_spawned", agent_identity: "helper", spawned_by: "billing" } }] },
    getMemoryOverview: { scopes: [{ scope_kind: "identity", scope_key: "billing", active: 3 }] },
  };

  test("reads routes, lineage and memory with the targets and parameters they need", async () => {
    const calls: Array<[string, string, unknown]> = [];
    const result = await readTopologyRelations({
      methods: Object.values(methods),
      methodOf: methodOf as never,
      execute: async (command, target, params) => {
        calls.push([command, target, params]);
        return answers[command];
      },
      memoryAvailable: true,
    });
    assert.deepEqual(calls.map(([command, target]) => `${command}@${target}`).sort(), [
      "getMemoryOverview@memory",
      "listDeliveryHistory@routing",
      "listRoutingRoutes@routing",
      "topologyMobEvents@topology",
    ]);
    assert.deepEqual(calls.find(([command]) => command === "topologyMobEvents")?.[2], { event_types: ["member_spawned"], limit: 2000 });
    assert.equal(result.routesAccess, "loaded");
    assert.equal(result.routes?.[0]?.managed, true);
    assert.deepEqual(result.lineage, [{ parent: "billing", child: "helper", kind: "spawn", atMs: null }]);
    assert.deepEqual(result.memoryScopes, [{ identity: "billing", records: 3, quarantined: 0 }]);
    assert.deepEqual(result.stores, [{ id: "team", label: "Team memory", records: 3 }]);
  });

  test("each read reaches the map as it lands: a slow lineage read holds back no other", async () => {
    let releaseLineage: () => void = () => undefined;
    const lineageGate = new Promise<void>((resolve) => { releaseLineage = resolve; });
    const partials: string[] = [];
    const done = readTopologyRelations({
      methods: Object.values(methods),
      methodOf: methodOf as never,
      execute: async (command) => {
        if (command === "topologyMobEvents") await lineageGate;
        return answers[command];
      },
      memoryAvailable: true,
    }, (partial) => { partials.push(Object.keys(partial).sort().join("+")); });
    // Every read but the lineage settles.
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(partials.sort(), ["memoryScopes+stores", "routes+routesAccess"]);
    releaseLineage();
    const result = await done;
    await new Promise((resolve) => setTimeout(resolve, 0));
    assert.deepEqual(partials.sort(), ["lineage", "memoryScopes+stores", "routes+routesAccess"]);
    assert.equal(result.lineage?.length, 1);
  });

  test("a refused routes read says why, a failed one leaves only its relation out, and nothing unadvertised is read", async () => {
    const calls: string[] = [];
    const result = await readTopologyRelations({
      methods: [methods.listRoutingRoutes, methods.topologyMobEvents],
      methodOf: methodOf as never,
      execute: async (command) => {
        calls.push(command);
        if (command === "listRoutingRoutes") throw Object.assign(new Error("denied"), { rpcError: { code: -32030, data: { kind: "access_denied" } } });
        throw new Error("lineage read failed");
      },
      memoryAvailable: false,
    });
    assert.deepEqual(calls.sort(), ["listRoutingRoutes", "topologyMobEvents"]);
    assert.deepEqual(result, { routes: null, routesAccess: "denied", lineage: null, memoryScopes: null, stores: null });
    const none = await readTopologyRelations({ methods: [], methodOf: methodOf as never, execute: async () => ({}), memoryAvailable: true });
    assert.deepEqual(none, { routes: null, routesAccess: "unavailable", lineage: null, memoryScopes: null, stores: null });
  });
});

describe("ledger backfill", () => {
  const frame = (seq: number, timestampMs: number) => ({
    id: `f${seq}`,
    event: "run_started",
    identity: "billing",
    timestampMs,
    cursor: `console:${seq}`,
    data: {},
  });

  test("pages back from the oldest frame read until the window is covered, and hands the ledger one batch", async () => {
    // A page lists its frames oldest first; the next page is the one before its oldest cursor.
    const pages: Record<string, ReturnType<typeof frame>[]> = {
      newest: [frame(8, 2_500), frame(9, 3_000)],
      "console:8": [frame(6, 900), frame(7, 2_000)],
      "console:6": [frame(5, 500)],
    };
    const asked: Array<string | undefined> = [];
    const batches: string[][] = [];
    const read = await backfillTopologyLedger({
      query: async (before) => { asked.push(before); return { frames: pages[before ?? "newest"]! }; },
      ingest: (frames) => { batches.push(frames.map((entry) => entry.id)); },
      sinceMs: 1_000,
    });
    // The second page reaches past the window's start: no third read.
    assert.deepEqual(asked, [undefined, "console:8"]);
    assert.deepEqual(batches, [["f8", "f9", "f6", "f7"]]);
    assert.equal(read, 4);
  });

  test("stops at an exhausted history, at the page cap, and at a page that reaches no further back", async () => {
    const batches: number[] = [];
    await backfillTopologyLedger({
      query: async () => ({ frames: [frame(1, 5_000)], exhausted: true }),
      ingest: (frames) => { batches.push(frames.length); },
      sinceMs: 0,
    });
    let calls = 0;
    await backfillTopologyLedger({
      query: async () => { calls += 1; return { frames: [frame(100 - calls, 5_000)] }; },
      ingest: (frames) => { batches.push(frames.length); },
      sinceMs: 0,
      maxPages: 3,
    });
    assert.equal(calls, 3);
    // A cursor the timeline ignores (the same page again) ends the backfill.
    let repeats = 0;
    await backfillTopologyLedger({
      query: async () => { repeats += 1; return { frames: [frame(40, 5_000), frame(41, 5_100)] }; },
      ingest: (frames) => { batches.push(frames.length); },
      sinceMs: 0,
    });
    assert.equal(repeats, 2);
    assert.deepEqual(batches, [1, 3, 4]);
    // Nothing read, nothing handed over.
    await backfillTopologyLedger({ query: async () => ({ frames: [] }), ingest: () => { batches.push(-1); }, sinceMs: 0 });
    assert.deepEqual(batches, [1, 3, 4]);
  });
});

test("wiring roles are the profiles that wire their members, one set per capabilities object", () => {
  const capabilities = {
    lead: { has_wiring: false, instance_count: 7 },
    coordinator: { has_wiring: true },
    specialist: { has_wiring: true, addressable: true },
    scribe: {},
  };
  const roles = topologyWiringRoles(capabilities);
  assert.deepEqual([...(roles ?? [])].sort(), ["coordinator", "specialist"]);
  assert.equal(topologyWiringRoles(capabilities), roles);
  assert.equal(topologyWiringRoles(undefined), null);
  assert.equal(topologyWiringRoles(null), null);
});
