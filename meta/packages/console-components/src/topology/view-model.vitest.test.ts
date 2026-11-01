import assert from "node:assert/strict";
import { describe, test } from "vitest";

import { buildGraph, type TopoAgent } from "./data";
import type { ConsoleAgent, ConsoleTopologyLink, ConsoleTopologyNode } from "./types";
import {
  buildTopologyView,
  fallbackTopologyStatus,
  topologyConnectionCount,
  topologyConnectionWords,
  topologySummaryFacts,
  topologyTrafficWindow,
  type TopologyViewOptions,
} from "./view-model";

const key = (identity: string, authority = "ops") => `mk1|${authority}|${identity}`;
const node = (identity: string, extra: Partial<ConsoleTopologyNode> = {}, authority = "ops"): ConsoleTopologyNode => ({
  identity: key(identity, authority),
  ref: { id: key(identity, authority), authority, identity },
  role: "specialist",
  labels: {},
  wired_to: [],
  ...extra,
});
const link = (a: string, b: string, flags: Partial<ConsoleTopologyLink>): ConsoleTopologyLink => ({
  from: key(a),
  to: key(b),
  actual: false,
  ...flags,
});
const agent = (identity: string, extra: Partial<ConsoleAgent> = {}): ConsoleAgent => ({
  identity,
  agent_id: identity,
  member_id: identity,
  label: identity[0]!.toUpperCase() + identity.slice(1),
  kind: "mob_agent",
  state: "active",
  ...extra,
});

const SECTIONS: Record<string, string> = {
  dispatch: "Coordinators",
  triage: "Coordinators",
  billing: "Domains",
  delivery: "Domains",
  support: "Domains",
  gate: "Internal",
};

function options(extra: Partial<TopologyViewOptions> = {}): TopologyViewOptions {
  return {
    statusOf: (entry: TopoAgent) => fallbackTopologyStatus(entry, false),
    sectionOf: (entry: TopoAgent) => SECTIONS[entry.id] || "Other",
    sectionOrder: ["Personal", "Coordinators", "Domains", "Internal", "Other"],
    ...extra,
  };
}

function team() {
  return buildGraph(
    [
      node("dispatch", { role: "coordinator" }),
      node("triage", { role: "coordinator" }),
      node("billing"),
      node("delivery"),
      node("support"),
      node("gate"),
      node("ticket-4411", { labels: { spawned_by: "support" } }),
      node("ledger", { presentation: { crossScope: true } }, "finance"),
    ],
    [
      agent("triage", { response_phase: "generating" }),
      agent("delivery", { progress: { health: "degraded", run_state: "idle", in_flight_work: 0, last_progress_at_ms: 0, last_progress_event: "unchanged" } }),
      agent("gate", { state: "broken" }),
    ],
    {
      links: [
        link("dispatch", "triage", { actual: true, declared: true, desired: true }),
        link("dispatch", "billing", { actual: true, declared: true, desired: true }),
        link("triage", "support", { actual: true, declared: true, desired: true }),
        link("billing", "gate", { declared: true, desired: true }),
        link("delivery", "gate", { declared: true, suppressed: true }),
        { from: key("billing"), to: key("ledger", "finance"), actual: false, desired: true, operatorAdded: true, cross: true },
      ],
      routes: [
        { key: "billing-events", channel: "email", recipient: "billing", deliveries: { sent: 3, failed: 1 } },
        ...["dispatch", "triage", "billing", "delivery", "support", "gate"].map((identity) => ({
          key: `mob.member.${identity}`,
          channel: "notification",
          recipient: identity,
          managed: true,
        })),
      ],
      traffic: [{ from: "triage", to: "support", count: 12, lastAtMs: 1_000 }],
      stores: [{ id: "team", label: "Team memory", records: 40 }],
    },
  );
}

describe("topology view model", () => {
  test("lanes follow the section order between channels and the shared and other-team lanes", () => {
    const view = buildTopologyView(team(), options());
    assert.deepEqual(view.lanes.map((lane) => lane.id), ["channels", "Coordinators", "Domains", "Internal", "shared", "other-teams"]);
    const byId = new Map(view.nodes.map((entry) => [entry.id, entry]));
    assert.equal(byId.get("dispatch")?.kind, "coordinator");
    assert.equal(byId.get("billing")?.kind, "agent");
    // A helper sits in its parent's lane, hung off it.
    assert.equal(byId.get("ticket-4411")?.kind, "helper");
    assert.equal(byId.get("ticket-4411")?.lane, "Domains");
    assert.equal(byId.get("ticket-4411")?.anchor, "support");
    assert.equal(byId.get("ledger@finance")?.kind, "external");
    assert.equal(byId.get("ledger@finance")?.lane, "other-teams");
    assert.equal(byId.get("channel:email")?.kind, "source");
    assert.equal(byId.get("channel:notification")?.sublabel, "every member");
    assert.equal(byId.get("channel:notification")?.label, "notification · every member");
    assert.equal(byId.get("channel:email")?.label, "email");
    assert.equal(byId.get("store:team")?.kind, "store");
    assert.equal(byId.get("store:team")?.lane, "shared");
  });

  test("live, health and the status words come from the status rule", () => {
    const view = buildTopologyView(team(), options());
    const byId = new Map(view.nodes.map((entry) => [entry.id, entry]));
    assert.equal(byId.get("triage")?.live, true);
    assert.equal(byId.get("triage")?.tone, "live");
    assert.equal(byId.get("triage")?.statusLabel, "Working");
    assert.equal(byId.get("delivery")?.badge, "warn");
    assert.equal(byId.get("delivery")?.statusLabel, "Degraded");
    assert.equal(byId.get("gate")?.badge, "crit");
    assert.equal(byId.get("gate")?.statusLabel, "Broken");
    assert.equal(byId.get("billing")?.statusLabel, "Idle");
  });

  test("every relation kind is drawn, a roster channel's fan only on demand", () => {
    const relations = (view: ReturnType<typeof buildTopologyView>) =>
      view.edges.map((edge) => `${edge.relation}:${edge.from}>${edge.to}:${edge.state}`);
    const view = buildTopologyView(team(), options());
    assert.deepEqual(relations(view), [
      "route:channel:email>billing:failed",
      "lineage:support>ticket-4411:normal",
      "wired:billing>dispatch:normal",
      "wired:billing>gate:declared",
      "wired:delivery>gate:declared",
      "wired:dispatch>triage:normal",
      "wired:support>triage:normal",
      "cross:billing>ledger@finance:normal",
      "traffic:triage>support:normal",
    ]);
    const traffic = view.edges.find((edge) => edge.relation === "traffic");
    assert.equal(traffic?.label, "12/h");
    assert.equal(traffic?.weight, 12);
    assert.equal(traffic?.directed, true);
    assert.equal(traffic?.title, "Triage to Support · 12 messages in the last hour · wired");
    // With the clock: when the newest of them was sent.
    const timed = buildTopologyView(team(), options({ now: 1_000 + 2 * 60_000 }));
    assert.equal(
      timed.edges.find((edge) => edge.relation === "traffic")?.title,
      "Triage to Support · 12 messages in the last hour · last 2m ago · wired",
    );
    assert.equal(view.edges.find((edge) => edge.id === "wired:delivery:gate")?.attributes?.["data-suppressed"], "true");
    // Hovering a member draws its own roster route; hovering the channel draws them all.
    const member = buildTopologyView(team(), options({ reveal: new Set(["gate"]) }));
    assert.ok(relations(member).includes("route:channel:notification>gate:normal"));
    assert.equal(relations(member).filter((entry) => entry.startsWith("route:channel:notification")).length, 1);
    const channel = buildTopologyView(team(), options({ reveal: new Set(["channel:notification"]) }));
    assert.equal(relations(channel).filter((entry) => entry.startsWith("route:channel:notification")).length, 6);
  });

  test("hidden helpers and channels take their edges with them; Focus keeps only a node's edges", () => {
    const view = buildTopologyView(team(), options({ showHelpers: false, showChannels: false }));
    assert.equal(view.nodes.some((entry) => entry.id === "ticket-4411" || entry.kind === "source"), false);
    assert.equal(view.edges.some((edge) => edge.relation === "lineage" || edge.relation === "route"), false);
    assert.equal(view.lanes.some((lane) => lane.id === "channels"), false);
    const focused = buildTopologyView(team(), options({ focus: new Set(["triage"]) }));
    assert.deepEqual(focused.edges.map((edge) => edge.id), [
      "wired:dispatch:triage",
      "wired:support:triage",
      "traffic:triage:support",
    ]);
  });

  test("figures, problems and the summary read the team", () => {
    const view = buildTopologyView(team(), options());
    assert.deepEqual(view.counts, {
      agents: 7,
      longLived: 6,
      helpers: 1,
      working: 1,
      otherTeams: 1,
      wired: 3,
      cross: 1,
      declared: 2,
      routes: 1,
      rosterRoutes: 6,
      lineage: 1,
      trafficPairs: 1,
    });
    // A roster channel's routes reach every member: not counted as connections.
    assert.equal(topologyConnectionCount(view.counts), 6);
    assert.equal(topologyConnectionWords(view.counts), "3 wired · 1 to other teams · 1 route · 1 helper · 2 not wired");
    assert.deepEqual(view.problems.map((problem) => [problem.tone, problem.text]), [
      ["crit", "Gate is broken"],
      ["crit", "1 delivery to Billing failed"],
      ["warn", "Delivery is degraded"],
      ["warn", "Billing and Gate are not wired"],
    ]);
    assert.deepEqual(topologySummaryFacts(view).map((fact) => [fact.text, fact.tone]), [
      ["7 agents", undefined],
      ["1 working now", "live"],
      ["1 broken", "crit"],
      ["1 delivery failed", "crit"],
    ]);
    assert.equal(view.hasRelations, true);
  });

  test("a team with no relation at all says none", () => {
    const graph = buildGraph([node("a"), node("b")], [], { links: [] });
    const view = buildTopologyView(graph, options());
    assert.equal(view.hasRelations, false);
    assert.equal(topologyConnectionWords(view.counts), "none");
    assert.equal(topologyConnectionCount(view.counts), 0);
    assert.deepEqual(view.lanes.map((lane) => lane.id), ["Other"]);
    // The runtime's own route to every member connects no one.
    const roster = buildGraph([node("a"), node("b")], [], {
      links: [],
      routes: ["a", "b"].map((identity) => ({ key: `mob.member.${identity}`, channel: "notification", recipient: identity, managed: true })),
    });
    const rosterView = buildTopologyView(roster, options());
    assert.equal(rosterView.hasRelations, false);
    assert.equal(topologyConnectionWords(rosterView.counts), "none");
  });
});

describe("topology traffic window", () => {
  test("sums the window by pair and bin, with the window before it", () => {
    const now = 10 * 60 * 60_000;
    const hour = 60 * 60_000;
    const result = topologyTrafficWindow(
      [
        { from: "a", to: "b", atMs: now - 5_000 },
        { from: "a", to: "b", atMs: now - 30 * 60_000, count: 3 },
        { from: "b", to: "a", atMs: now - 59 * 60_000 },
        // The hour before.
        { from: "a", to: "b", atMs: now - hour - 60_000, count: 2 },
        // Older than both windows.
        { from: "a", to: "b", atMs: now - 3 * hour },
        { from: "a", to: "a", atMs: now - 1_000 },
      ],
      now,
      hour,
    );
    assert.equal(result.total, 5);
    assert.equal(result.previousTotal, 2);
    assert.deepEqual(result.traffic, [
      { from: "a", to: "b", count: 4, lastAtMs: now - 5_000 },
      { from: "b", to: "a", count: 1, lastAtMs: now - 59 * 60_000 },
    ]);
    assert.equal(result.bins.length, 12);
    assert.deepEqual(result.bins, [1, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0, 1]);
  });
});
