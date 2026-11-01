import assert from "node:assert/strict";
import { describe, test } from "vitest";

import type { TopologyOperationReceipt } from "@console-core";

import { buildGraph, deriveTopologyActivity, deriveTopologyTraffic, topologyReferenceMatchesKey } from "./data";
import type { ConsoleAgent, ConsoleFrame, ConsoleTopologyLink, ConsoleTopologyNode } from "./types";

const key = (authority: string, identity: string) => `mk1|${encodeURIComponent(authority)}|${encodeURIComponent(identity)}`;

function agent(identity: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent {
  return {
    identity,
    agent_id: identity,
    member_id: identity,
    label: identity[0]!.toUpperCase() + identity.slice(1),
    kind: "mob_agent",
    role: "specialist",
    state: "active",
    wired_to: [],
    ...extra,
  };
}

/** A node the way the topology query adapter shapes it (stock endpoint keys). */
function queryNode(identity: string, extra: Partial<ConsoleTopologyNode> = {}, authority = "ops"): ConsoleTopologyNode {
  const id = key(authority, identity);
  return { identity: id, ref: { id, authority, identity }, role: "specialist", labels: {}, wired_to: [], ...extra };
}

function link(a: string, b: string, flags: Partial<ConsoleTopologyLink>, authority = "ops", bAuthority = authority): ConsoleTopologyLink {
  return { from: key(authority, a), to: key(bAuthority, b), actual: false, ...flags };
}

const relations = (graph: ReturnType<typeof buildGraph>) =>
  graph.edges.map((edge) => `${edge.relation}:${edge.from}>${edge.to}:${edge.state}`);

describe("topology graph model", () => {
  test("merges the query's nodes with the roster and keys every node by its identity", () => {
    const graph = buildGraph(
      [queryNode("billing"), queryNode("dispatch", { role: "coordinator" })],
      [
        agent("billing", { label: "Billing", group: "Domain", response_phase: "generating" }),
        agent("dispatch", { label: "Dispatch", group: "Coordinators" }),
        // The roster lists an agent the query left out: it is still on the map.
        agent("insights", { label: "Insights", group: "Specialists" }),
      ],
      { links: [] },
    );
    assert.deepEqual(graph.agents.map((entry) => entry.id), ["billing", "dispatch", "insights"]);
    // The endpoint keys stay on the ref, for connect and disconnect.
    assert.equal(graph.byId.get("billing")?.ref.id, key("ops", "billing"));
    assert.equal(graph.byId.get("insights")?.ref.id, "insights");
    assert.equal(graph.byId.get("billing")?.group, "Domain");
    assert.equal(graph.byId.get("billing")?.responsePhase, "generating");
    assert.equal(graph.byId.get("dispatch")?.role, "coordinator");
    // Every reference form resolves to the same node.
    for (const reference of ["billing", key("ops", "billing"), "ops/specialist/billing", "BILLING"]) {
      assert.equal(graph.resolve(reference), "billing", reference);
    }
  });

  test("names a nameless agent the way every surface does", () => {
    const graph = buildGraph([
      { identity: "router:main", role: "router", state: "active", wired_to: [], labels: {} },
      queryNode("identity:queue-agent", { role: "worker" }),
    ], []);
    assert.equal(graph.byId.get("router:main")?.label, "Router main");
    assert.equal(graph.byId.get("identity:queue-agent")?.label, "Queue agent");
    const named = buildGraph([{ identity: "router:main", label: "Front Desk", wired_to: [] }], []);
    assert.equal(named.byId.get("router:main")?.label, "Front Desk");
  });

  test("without the query, every node source's wired_to counts and aliases resolve", () => {
    const graph = buildGraph(
      [
        { identity: "commander", label: "Commander", wired_to: ["mob/coordination/triage"] },
        { identity: "triage", label: "Triage", wired_to: [] },
      ],
      [
        // The roster's own wiring is merged, not ignored because nodes exist.
        agent("responder", { member_id: "member-responder", wired_to: ["Commander"] }),
        agent("scribe", { wired_to: ["member-responder", "nobody-we-know"] }),
      ],
    );
    assert.deepEqual(relations(graph), [
      "wired:commander>responder:normal",
      "wired:commander>triage:normal",
      "wired:responder>scribe:normal",
    ]);
    assert.deepEqual(graph.byId.get("commander")?.wiredTo, ["responder", "triage"]);
    assert.equal(graph.degree.commander, 2);
    assert.equal(graph.degree.scribe, 1);
  });

  test("the query's links decide the wiring and keep declared, suppressed and cross pairs", () => {
    const nodes = [
      queryNode("billing", { wired_to: [key("ops", "dispatch")] }),
      queryNode("dispatch"),
      queryNode("returns"),
      queryNode("insights"),
      queryNode("shipping"),
      queryNode("ledger", { presentation: { crossScope: true, scopeLabel: "finance" } }, "finance"),
    ];
    const graph = buildGraph(nodes, [agent("returns", { wired_to: ["insights"] })], {
      links: [
        link("billing", "dispatch", { actual: true, declared: true, desired: true }),
        link("billing", "returns", { declared: true, desired: true }),
        link("insights", "returns", { declared: true, suppressed: true }),
        link("dispatch", "shipping", { actual: true, suppressed: true, declared: true }),
        link("billing", "ledger", { desired: true, operatorAdded: true, cross: true }, "ops", "finance"),
      ],
    });
    // The roster's one-sided returns>insights is not wired: the query says the pair was suppressed.
    assert.deepEqual(relations(graph), [
      "wired:billing>dispatch:normal",
      "wired:billing>returns:declared",
      "wired:dispatch>shipping:normal",
      "wired:insights>returns:declared",
      "cross:billing>ledger@finance:normal",
    ]);
    const edge = (id: string) => graph.edges.find((candidate) => candidate.id === id);
    assert.equal(edge("wired:billing:returns")?.degraded, true);
    assert.equal(edge("wired:insights:returns")?.suppressed, true);
    assert.equal(edge("wired:insights:returns")?.degraded, undefined);
    assert.equal(edge("wired:dispatch:shipping")?.conflict, true);
    assert.equal(edge("cross:billing:ledger@finance")?.operatorAdded, true);
    // Mutations act on the endpoint keys.
    assert.deepEqual(edge("wired:billing:dispatch")?.endpoints, { from: key("ops", "billing"), to: key("ops", "dispatch") });
    const ledger = graph.byId.get("ledger@finance");
    assert.equal(ledger?.kind, "external");
    assert.equal(ledger?.external, true);
    // A bare identity never resolves to the other team's agent.
    assert.equal(graph.resolve("ledger"), null);
    assert.equal(graph.resolve("finance/accounts/ledger"), "ledger@finance");
    assert.equal(graph.resolve("marketing/specialist/billing"), null);
    assert.deepEqual(graph.byId.get("billing")?.wiredTo, ["dispatch", "ledger@finance"]);
  });

  test("a connect or disconnect in flight turns its pair pending, and a new connect draws one", () => {
    const operation = (action: "connect" | "disconnect", from: string, to: string, status = "running"): TopologyOperationReceipt => ({
      operationId: `${action}-${from}-${to}`,
      action,
      status: status as TopologyOperationReceipt["status"],
      edge: { from: key("ops", from), to: key("ops", to) },
    });
    const graph = buildGraph(
      [queryNode("a"), queryNode("b"), queryNode("c"), queryNode("d")],
      [],
      {
        links: [link("a", "b", { actual: true, desired: true })],
        operations: [
          operation("disconnect", "a", "b"),
          operation("connect", "c", "d"),
          operation("connect", "a", "c", "succeeded"),
          operation("disconnect", "b", "d"),
        ],
      },
    );
    assert.deepEqual(relations(graph), ["wired:a>b:pending", "wired:c>d:pending"]);
    assert.equal(graph.edges[0]?.pendingAction, "disconnect");
    assert.equal(graph.edges[1]?.pendingAction, "connect");
  });

  test("helpers hang off their parent: typed lineage first, then the labels", () => {
    const graph = buildGraph(
      [
        queryNode("billing"),
        queryNode("billing-refund-review", { labels: { side_chat_of: "billing" } }),
        queryNode("ticket-4411", { labels: { spawned_by: "support" } }),
        queryNode("support"),
        queryNode("draft-writer"),
      ],
      [agent("notes", { side_chat_of: "support" })],
      {
        links: [],
        lineage: [{ parent: "ops/specialist/support", child: "draft-writer", kind: "fork", atMs: 5 }],
      },
    );
    assert.deepEqual(relations(graph).filter((entry) => entry.startsWith("lineage")), [
      "lineage:billing>billing-refund-review:normal",
      "lineage:support>draft-writer:normal",
      "lineage:support>notes:normal",
      "lineage:support>ticket-4411:normal",
    ]);
    assert.equal(graph.byId.get("billing-refund-review")?.kind, "helper");
    assert.equal(graph.byId.get("billing-refund-review")?.lineage, "side_chat");
    assert.equal(graph.byId.get("draft-writer")?.lineage, "fork");
    assert.equal(graph.byId.get("ticket-4411")?.parent, "support");
    assert.equal(graph.byId.get("notes")?.lineage, "side_chat");
    assert.equal(graph.edges.find((edge) => edge.relation === "lineage")?.directed, true);
  });

  test("routes feed their recipients from one source node per channel", () => {
    const graph = buildGraph([queryNode("billing"), queryNode("support")], [], {
      links: [],
      routes: [
        { key: "billing-events", channel: "email", recipient: "billing", deliveries: { sent: 4, failed: 1, lastAtMs: 10 } },
        { key: "support-inbox", channel: "email", recipient: "ops/support" },
        { key: "mob.member.billing", channel: "notification", recipient: "billing", managed: true },
        { key: "mob.member.support", channel: "notification", recipient: "support", managed: true },
        // An outside address is not on the map.
        { key: "finance", channel: "email", recipient: "finance-lead@example.com" },
      ],
    });
    assert.deepEqual(graph.sources.map((source) => [source.id, source.recipients, source.managed]), [
      ["channel:email", ["billing", "support"], false],
      ["channel:notification", ["billing", "support"], true],
    ]);
    assert.deepEqual(relations(graph).filter((entry) => entry.startsWith("route")), [
      "route:channel:email>billing:failed",
      "route:channel:email>support:normal",
      "route:channel:notification>billing:normal",
      "route:channel:notification>support:normal",
    ]);
    const failed = graph.edges.find((edge) => edge.id === "route:channel:email:billing");
    assert.equal(failed?.failedDeliveries, 1);
    assert.equal(failed?.sentDeliveries, 4);
    assert.equal(failed?.directed, true);
    // Routes are not comms: they leave the degree alone.
    assert.equal(graph.degree.billing, 0);
  });

  test("traffic weights by window and direction, and a wired pair says so", () => {
    const graph = buildGraph([queryNode("triage"), queryNode("support"), queryNode("router")], [], {
      links: [link("support", "triage", { actual: true, desired: true })],
      traffic: [
        { from: "triage", to: "support", count: 12, lastAtMs: 900 },
        { from: "router", to: "support", count: 2, lastAtMs: 400 },
        { from: "support", to: "router", count: 1, lastAtMs: 500 },
        { from: "router", to: "nobody", count: 3 },
      ],
    });
    const traffic = graph.edges.filter((edge) => edge.relation === "traffic");
    assert.deepEqual(traffic.map((edge) => [edge.id, edge.weight, edge.directed, edge.wired]), [
      ["traffic:router:support", 3, false, false],
      ["traffic:triage:support", 12, true, true],
    ]);
    assert.deepEqual(traffic[0]?.counts, { forward: 2, backward: 1 });
    assert.equal(traffic[1]?.lastAtMs, 900);
    assert.equal(graph.degree.router, 1);
    assert.equal(graph.degree.support, 2);
  });
});

describe("topology traffic from frames", () => {
  const graph = buildGraph(
    [queryNode("dispatch"), queryNode("qa-review"), queryNode("billing"), queryNode("returns")],
    [],
    { links: [] },
  );
  const comms = (identity: string, at: number, kind: string, peer: string, peerId: string, deliveryId: string): ConsoleFrame => ({
    id: `notice-${deliveryId}-${identity}`,
    event: "system_notice",
    identity,
    timestampMs: at,
    data: {
      kind: "comms",
      blocks: [{ type: "comms", kind, direction: "incoming", peer: { display_name: peer, id: peerId }, delivery_id: deliveryId }],
      message: { blocks: [{ type: "comms", kind, direction: "incoming", peer: { display_name: peer, id: peerId }, delivery_id: deliveryId }] },
    },
  });

  test("counts each delivered message once and never a lifecycle notice", () => {
    const frames: ConsoleFrame[] = [
      comms("qa-review", 1_000, "message", "ops/coordinator/dispatch", "uuid-dispatch", "d1"),
      // The same delivery as an ingested peer message.
      {
        id: "ingested-d1",
        event: "peer_content_ingested",
        identity: "qa-review",
        timestampMs: 1_004,
        data: { kind: "message", delivery_id: "d1", peer: { display_name: "ops/coordinator/dispatch", id: "uuid-dispatch" } },
      },
      // The sender's own tool call for that message.
      {
        id: "call-30",
        event: "tool_call_requested",
        identity: "dispatch",
        timestampMs: 990,
        data: { name: "send_message", tool_call_id: "call-30", args: { peer_id: "uuid-qa", body: "Please review" } },
      },
      // A kickoff notice teaches the registry the reviewer's peer id; it is not a message.
      comms("dispatch", 500, "lifecycle", "ops/reviewer/qa-review", "uuid-qa", "kickoff-1"),
      comms("returns", 2_000, "message", "ops/specialist/billing", "uuid-billing", "d2"),
      comms("returns", 2_500, "request", "ops/specialist/billing", "uuid-billing", "d3"),
    ];
    const result = deriveTopologyTraffic(frames, graph.resolve, 0);
    assert.deepEqual(
      result.traffic.map((entry) => [entry.from, entry.to, entry.count, entry.lastAtMs]),
      [["dispatch", "qa-review", 1, 1_000], ["billing", "returns", 2, 2_500]],
    );
    assert.deepEqual(result.messages.map((message) => `${message.from}>${message.to}`), [
      "dispatch>qa-review",
      "billing>returns",
      "billing>returns",
    ]);
  });

  test("a sent message whose delivery the console did not see still counts, inside the window only", () => {
    const frames: ConsoleFrame[] = [
      comms("dispatch", 100, "lifecycle", "ops/reviewer/qa-review", "uuid-qa", "kickoff-1"),
      {
        id: "call-1",
        event: "tool_call_requested",
        identity: "dispatch",
        timestampMs: 5_000,
        data: { name: "send_message", tool_call_id: "call-1", args: { peer_id: "uuid-qa" } },
      },
      // The session-history copy of the same call.
      {
        id: "call-1-history",
        event: "tool_call_requested",
        identity: "dispatch",
        timestampMs: 5_001,
        data: { name: "send_message", tool_call_id: "call-1", args: { peer_id: "uuid-qa" } },
      },
      {
        id: "call-old",
        event: "tool_call_requested",
        identity: "billing",
        timestampMs: 10,
        data: { name: "send_message", tool_call_id: "call-old", args: { to: "returns" } },
      },
    ];
    const result = deriveTopologyTraffic(frames, graph.resolve, 1_000);
    assert.deepEqual(result.traffic, [{ from: "dispatch", to: "qa-review", count: 1, lastAtMs: 5_000 }]);
  });

  test("a message from an unknown peer is left out", () => {
    const frames = [comms("dispatch", 1_000, "message", "elsewhere/x/stranger", "uuid-x", "d9")];
    assert.deepEqual(deriveTopologyTraffic(frames, graph.resolve, 0).traffic, []);
  });

  test("activity joins frames on the node's identity even when the node is keyed by its endpoint", () => {
    const frames: ConsoleFrame[] = [
      { id: "3", event: "interaction_complete", identity: "billing", timestampMs: 30, data: {} },
      { id: "2", event: "run_started", identity: "billing", timestampMs: 20, data: {} },
      { id: "1", event: "run_started", identity: "dispatch", timestampMs: 10, data: {} },
    ];
    const activity = deriveTopologyActivity(frames, graph);
    assert.deepEqual(activity.busy, { billing: false, dispatch: true });
    assert.deepEqual(activity.active, { billing: 30, dispatch: 10 });
  });

  test("the fuzzy reference match is the sidebar's", () => {
    assert.equal(topologyReferenceMatchesKey("ops/coordination/triage", "triage"), true);
    assert.equal(topologyReferenceMatchesKey("Queue Agent", "queue-agent"), true);
    assert.equal(topologyReferenceMatchesKey("identity:review:singleton", "review:singleton"), true);
    assert.equal(topologyReferenceMatchesKey("ops/billing", "bill"), false);
  });
});
