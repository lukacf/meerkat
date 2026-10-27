import assert from "node:assert/strict";
import test from "node:test";
import {
  buildGraph,
  deriveTopologyActivity,
  graphStats,
  groupMatrix,
  groupSummaries,
  sampleEdges,
} from "./data";
import type { ConsoleAgent, ConsoleFrame, ConsoleTopologyNode } from "../../types";

test("topology graph preserves group labels and computes scale stats", () => {
  const agents: ConsoleAgent[] = [
    {
      identity: "a",
      agent_id: "a",
      member_id: "a",
      label: "Atlas A",
      kind: "agent",
      role: "coordinator",
      state: "active",
      response_phase: "generating",
      wired_to: ["b", "c"],
      labels: { console_group: "Atlas" },
    },
    {
      identity: "b",
      agent_id: "b",
      member_id: "b",
      label: "Atlas B",
      kind: "agent",
      role: "worker",
      state: "active",
      wired_to: ["a"],
      labels: { console_group: "Atlas" },
    },
    {
      identity: "c",
      agent_id: "c",
      member_id: "c",
      label: "Borealis C",
      kind: "agent",
      role: "worker",
      state: "active",
      wired_to: ["a"],
      labels: { console_group: "Borealis" },
    },
  ];
  const graph = buildGraph([], agents);

  assert.equal(graph.agents.length, 3);
  assert.equal(graph.edges.length, 2);
  assert.deepEqual(graph.groups, ["Atlas", "Borealis"]);
  assert.equal(graph.byId.get("a")?.group, "Atlas");
  assert.equal(graph.byId.get("a")?.responsePhase, "generating");

  const stats = graphStats(graph);
  assert.equal(stats.nodeCount, 3);
  assert.equal(stats.edgeCount, 2);
  assert.equal(stats.minDegree, 1);
  assert.equal(stats.maxDegree, 2);
  assert.equal(stats.avgDegree, 4 / 3);

  assert.deepEqual(groupSummaries(graph), [
    { group: "Atlas", count: 2, internalEdges: 1, externalEdges: 1 },
    { group: "Borealis", count: 1, internalEdges: 0, externalEdges: 1 },
  ]);
  assert.deepEqual(groupMatrix(graph), [
    { from: "Atlas", to: "Atlas", edges: 1 },
    { from: "Atlas", to: "Borealis", edges: 1 },
  ]);
});

test("topology graph uses topology node labels over registry fallback", () => {
  const nodes: ConsoleTopologyNode[] = [
    {
      identity: "node-a",
      label: "Projected A",
      role: "projected",
      state: "active",
      wired_to: [],
      labels: { console_group: "Projected" },
    },
  ];
  const agents: ConsoleAgent[] = [
    {
      identity: "node-a",
      agent_id: "node-a",
      member_id: "node-a",
      label: "Registry A",
      kind: "agent",
      role: "registry",
      state: "idle",
      wired_to: [],
      labels: { console_group: "Registry" },
    },
  ];

  const graph = buildGraph(nodes, agents);
  assert.equal(graph.byId.get("node-a")?.label, "Projected A");
  assert.equal(graph.byId.get("node-a")?.role, "projected");
  assert.equal(graph.byId.get("node-a")?.group, "Projected");
});

test("edge sampling is deterministic and bounded", () => {
  const edges = Array.from({ length: 10 }, (_, i) => ({ from: `a${i}`, to: `b${i}` }));
  assert.deepEqual(sampleEdges(edges, 3), [
    { from: "a0", to: "b0" },
    { from: "a3", to: "b3" },
    { from: "a6", to: "b6" },
  ]);
  assert.equal(sampleEdges(edges, 20).length, 10);
  assert.equal(sampleEdges(edges, 0).length, 0);
});

test("topology activity derives working nodes and peer call pulses", () => {
  const agents: ConsoleAgent[] = [
    {
      identity: "commander",
      agent_id: "commander",
      member_id: "commander-member",
      label: "Commander",
      kind: "agent",
      role: "coordinator",
      state: "active",
      wired_to: ["scribe"],
    },
    {
      identity: "scribe",
      agent_id: "scribe",
      member_id: "scribe-member",
      label: "Scribe",
      kind: "agent",
      role: "worker",
      state: "active",
      wired_to: ["commander"],
    },
  ];
  const graph = buildGraph([], agents);
  const frames: ConsoleFrame[] = [
    {
      id: "send",
      event: "tool_call_requested",
      identity: "commander",
      timestampMs: 1_100,
      data: {
        id: "call-send",
        name: "send_message",
        args: { peer_id: "peer-scribe" },
      },
    },
    {
      id: "peers",
      event: "tool_execution_completed",
      identity: "commander",
      timestampMs: 1_000,
      data: {
        id: "call-peers",
        name: "peers",
        result: JSON.stringify({
          peers: [{ peer_id: "peer-scribe", name: "mob/scribe" }],
        }),
      },
    },
    {
      id: "started",
      event: "interaction_started",
      identity: "commander",
      timestampMs: 900,
      data: {},
    },
  ];

  const activity = deriveTopologyActivity(frames, graph, 1_200, 1_000);
  assert.equal(activity.busy.commander, true);
  assert.equal(activity.active.commander, 1_100);
  assert.equal(activity.calls.commander, 1_100);
  assert.equal(activity.calls.scribe, 1_100);
  assert.deepEqual(activity.pulses.map((p) => [p.from, p.to]), [["commander", "scribe"]]);
});

test("topology activity derives peer call pulses from session-history assistant tool-use blocks", () => {
  const agents: ConsoleAgent[] = [
    {
      identity: "review-worker-1",
      agent_id: "review-worker-1",
      member_id: "review-worker-1",
      label: "Review Worker 1",
      kind: "agent",
      role: "delegate",
      state: "active",
      wired_to: ["initiative:daily-comet"],
    },
    {
      identity: "initiative:daily-comet",
      agent_id: "initiative:daily-comet",
      member_id: "initiative:daily-comet",
      label: "Daily Comet",
      kind: "agent",
      role: "initiative",
      state: "active",
      wired_to: ["review-worker-1"],
    },
  ];
  const graph = buildGraph([], agents);
  const frames: ConsoleFrame[] = [
    {
      id: "history-send",
      event: "interaction_complete",
      identity: "review-worker-1",
      timestampMs: 2_000,
      data: {
        message: {
          blocks: [
            {
              block_type: "tool_use",
              data: {
                id: "call-send",
                name: "send_message",
                args: { target_identity: "initiative:daily-comet" },
              },
            },
          ],
        },
      },
    },
  ];

  const activity = deriveTopologyActivity(frames, graph, 2_100, 1_000);
  assert.deepEqual(activity.pulses.map((p) => [p.from, p.to]), [
    ["review-worker-1", "initiative:daily-comet"],
  ]);
  assert.equal(activity.calls["review-worker-1"], 2_000);
  assert.equal(activity.calls["initiative:daily-comet"], 2_000);
});

test("topology activity derives pulses from typed incoming comms notices", () => {
  const agents: ConsoleAgent[] = [
    {
      identity: "review:singleton",
      agent_id: "review:singleton",
      member_id: "review:singleton",
      label: "Review Agent",
      kind: "agent",
      role: "review",
      state: "active",
      wired_to: ["review-worker-daily-comet"],
    },
    {
      identity: "review-worker-daily-comet",
      agent_id: "review-worker-daily-comet",
      member_id: "review-worker-daily-comet",
      label: "Daily Comet Worker",
      kind: "agent",
      role: "review-worker",
      state: "active",
      wired_to: ["review:singleton"],
    },
  ];
  const graph = buildGraph([], agents);
  const frames: ConsoleFrame[] = [
    {
      id: "typed-comms",
      event: "system_notice",
      identity: "review:singleton",
      timestampMs: 2_000,
      data: {
        message: {
          role: "system_notice",
          kind: "comms",
          body: "Received worker result",
          blocks: [{
            type: "comms",
            kind: "message",
            direction: "incoming",
            peer: {
              id: "ops/review-worker/review-worker-daily-comet",
              display_name: "ops/review-worker/review-worker-daily-comet",
            },
            request_id: "peer-result-1",
            content: [{ type: "text", text: "Worker result" }],
          }],
        },
      },
    },
  ];

  const activity = deriveTopologyActivity(frames, graph, 2_050, 1_000);
  assert.deepEqual(
    activity.pulses.map((p) => [p.from, p.to]),
    [["review-worker-daily-comet", "review:singleton"]],
  );
  assert.equal(activity.calls["review-worker-daily-comet"], 2_000);
  assert.equal(activity.calls["review:singleton"], 2_000);
});
