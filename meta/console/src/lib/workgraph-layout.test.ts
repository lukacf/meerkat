import assert from "node:assert/strict";
import test from "node:test";

import {
  WORKGRAPH_GRAPH_NODE_CAP,
  WORKGRAPH_LARGE_ITEM_COUNT,
  WORKGRAPH_OTHER_FRAME_ID,
  buildWorkGraphGraph,
  buildWorkGraphModel,
  workGraphActivityDays,
  workGraphAge,
  workGraphBlockedFoot,
  workGraphBlockerResolved,
  workGraphCardMeta,
  workGraphCriticalPath,
  workGraphDefaultView,
  workGraphDoneBy,
  workGraphEdgeWord,
  workGraphStatusWord,
} from "./workgraph-layout";
import type { WorkGraphWireEdge, WorkGraphWireItem } from "../types";

const NOW = Date.parse("2026-07-08T12:00:00Z");

function item(id: string, createdAt: string, extra: Partial<WorkGraphWireItem> = {}): WorkGraphWireItem {
  return { id, title: id, created_at: createdAt, status: "open", revision: 1, ...extra };
}

function at(minute: number): string {
  return new Date(Date.parse("2026-07-08T08:00:00Z") + minute * 60_000).toISOString();
}

const parent = (child: string, of: string): WorkGraphWireEdge => ({ kind: "parent", from_id: child, to_id: of });
const blocks = (from: string, to: string): WorkGraphWireEdge => ({ kind: "blocks", from_id: from, to_id: to });

// ── Blockers and readiness ───────────────────────────────────────────────

test("only a present and completed blocker resolves", () => {
  assert.equal(workGraphBlockerResolved(item("a", at(0), { status: "completed" })), true);
  for (const status of ["open", "in_progress", "blocked", "cancelled", "failed"]) {
    assert.equal(workGraphBlockerResolved(item("a", at(0), { status })), false, status);
  }
  assert.equal(workGraphBlockerResolved(undefined), false, "an absent blocker still blocks");

  const model = buildWorkGraphModel({
    items: [
      item("done", at(0), { status: "completed" }),
      item("dropped", at(1), { status: "cancelled" }),
      item("broke", at(2), { status: "failed" }),
      item("target", at(3)),
    ],
    edges: [blocks("done", "target"), blocks("dropped", "target"), blocks("broke", "target"), blocks("ghost", "target")],
    readyItemIds: null,
    now: NOW,
  });
  const target = model.items.get("target")!;
  assert.deepEqual(target.blockerIds, ["done", "dropped", "broke", "ghost"]);
  assert.deepEqual(target.unresolvedBlockerIds, ["dropped", "broke", "ghost"], "cancelled, failed and absent blockers keep blocking");
  assert.equal(target.ready, false);
});

test("ready comes from the server's ready ids, cross-checked against blockers and the stored count", () => {
  const items = [
    item("free", at(0)),
    item("waiting", at(1)),
    item("blocker", at(2), { status: "in_progress" }),
    item("stale", at(3), { machine_state: { unresolved_blocker_count: 2 } }),
  ];
  const edges = [blocks("blocker", "waiting")];
  const model = buildWorkGraphModel({ items, edges, readyItemIds: ["free", "waiting", "stale"], now: NOW });
  assert.equal(model.readySource, "server");
  assert.equal(model.items.get("free")!.ready, true);
  assert.equal(model.items.get("waiting")!.ready, false, "an unresolved blocker in the snapshot overrides the set");
  assert.equal(model.items.get("stale")!.ready, false, "a stored unresolved count overrides the set");
  assert.equal(model.items.get("blocker")!.ready, false, "items outside the set are not ready");
  assert.equal(model.counts.ready, 1);

  // An empty set is an answer, not a missing field: nothing is ready.
  const none = buildWorkGraphModel({ items, edges, readyItemIds: [], now: NOW });
  assert.equal(none.readySource, "server");
  assert.equal(none.counts.ready, 0);
});

test("without ready ids the client rule applies: open, unblocked, children done, time window open", () => {
  const model = buildWorkGraphModel({
    items: [
      item("plain", at(0)),
      item("due-later", at(1), { due_at: "2026-07-10T00:00:00Z" }),
      item("due-past", at(2), { due_at: "2026-07-01T00:00:00Z" }),
      item("snoozed", at(3), { snoozed_until: "2026-07-09T00:00:00Z" }),
      item("not-before", at(4), { not_before: "2026-07-09T00:00:00Z" }),
      item("goal", at(5)),
      item("child", at(6), { status: "in_progress" }),
      item("running", at(7), { status: "in_progress" }),
    ],
    edges: [parent("child", "goal")],
    now: NOW,
  });
  assert.equal(model.readySource, "client");
  const ready = [...model.items.values()].filter((entry) => entry.ready).map((entry) => entry.id);
  assert.deepEqual(ready, ["plain", "due-past"]);
});

// ── Critical path ────────────────────────────────────────────────────────

test("the critical path is the longest chain of unfinished items over blocks edges", () => {
  const items = [
    item("freeze", at(0), { status: "completed" }),
    item("regression", at(1), { status: "in_progress" }),
    item("publish", at(2), { status: "blocked" }),
    item("announce", at(3)),
    item("notes", at(4)),
  ];
  const edges = [blocks("freeze", "regression"), blocks("regression", "publish"), blocks("publish", "announce"), blocks("notes", "announce")];
  const byId = new Map(items.map((entry) => [entry.id!, entry]));
  const path = workGraphCriticalPath(items.map((entry) => entry.id!), byId, edges);
  assert.deepEqual(path.itemIds, ["regression", "publish", "announce"], "the completed head is not a step to finish");
  assert.deepEqual(path.edgeIds, ["blocks:regression:publish", "blocks:publish:announce"]);

  const model = buildWorkGraphModel({ items, edges, now: NOW });
  assert.deepEqual(model.criticalPath, path);
  assert.deepEqual([...model.items.values()].filter((entry) => entry.critical).map((entry) => entry.id), ["regression", "publish", "announce"]);
});

test("critical path ties go to more priority, then to the chain that starts with the older item", () => {
  const order = ["a1", "a2", "b1", "b2", "c1", "c2"];
  const make = (priorities: Record<string, string>) => new Map(order.map((id, index) =>
    [id, item(id, at(index), { priority: priorities[id] ?? "medium" })]));
  const edges = [blocks("a1", "a2"), blocks("b1", "b2"), blocks("c1", "c2")];
  // All equal: the oldest start wins.
  assert.deepEqual(workGraphCriticalPath(order, make({}), edges).itemIds, ["a1", "a2"]);
  // More priority wins over age.
  assert.deepEqual(workGraphCriticalPath(order, make({ c2: "high" }), edges).itemIds, ["c1", "c2"]);
  // Low priority loses a tie it would otherwise win by age.
  assert.deepEqual(workGraphCriticalPath(order, make({ a1: "low" }), edges).itemIds, ["b1", "b2"]);
  // Length always beats priority.
  const longer = [...edges, blocks("a2", "b1")];
  assert.deepEqual(workGraphCriticalPath(order, make({ c1: "high", c2: "high" }), longer).itemIds, ["a1", "a2", "b1", "b2"]);
});

test("critical path skips cancelled items, keeps failed ones and survives blocks cycles", () => {
  const items = new Map([
    ["x", item("x", at(0), { status: "failed" })],
    ["y", item("y", at(1), { status: "cancelled" })],
    ["z", item("z", at(2))],
  ]);
  assert.deepEqual(workGraphCriticalPath(["x", "y", "z"], items, [blocks("x", "y"), blocks("y", "z")]).itemIds, ["x"]);
  assert.deepEqual(workGraphCriticalPath(["x", "y", "z"], items, [blocks("x", "z")]).itemIds, ["x", "z"]);

  const loop = new Map([["p", item("p", at(0))], ["q", item("q", at(1))], ["r", item("r", at(2))]]);
  const cycle = [blocks("p", "q"), blocks("q", "r"), blocks("r", "p")];
  const first = workGraphCriticalPath(["p", "q", "r"], loop, cycle);
  assert.equal(first.itemIds.length, 3, "a cycle is cut once, keeping the longest walk");
  assert.deepEqual(workGraphCriticalPath(["p", "q", "r"], loop, [...cycle].reverse()), first, "deterministic in edge order");
  assert.deepEqual(workGraphCriticalPath([], new Map(), []), { itemIds: [], edgeIds: [] });
});

test("a long chain does not overflow the stack", () => {
  const ids = Array.from({ length: 5000 }, (_, index) => `n${index}`);
  const byId = new Map(ids.map((id, index) => [id, item(id, at(index))]));
  const edges = ids.slice(1).map((id, index) => blocks(ids[index], id));
  assert.equal(workGraphCriticalPath(ids, byId, edges).itemIds.length, 5000);
});

// ── Frames ───────────────────────────────────────────────────────────────

function releaseFixture() {
  const items = [
    item("release", at(0), { title: "Ship the release" }),
    item("freeze", at(1), { status: "completed" }),
    item("regression", at(2), { status: "in_progress", priority: "high", claim: { owner: { key: { kind: "agent", id: "research-analyst" } } } }),
    item("publish", at(3), { status: "blocked", priority: "high" }),
    item("announce", at(4), { priority: "low" }),
    item("docs", at(5), { title: "Docs" }),
    item("docs-api", at(6)),
    item("docs-api-examples", at(7)),
    item("vendor", at(8), { title: "Vendor review" }),
    item("scorecards", at(9), { status: "in_progress" }),
    item("loose", at(10), { title: "Rotate the keys" }),
    item("old-goal", at(11), { title: "Q3 follow-ups", status: "completed" }),
    item("old-1", at(12), { status: "completed" }),
    item("old-2", at(13), { status: "completed" }),
  ];
  const edges = [
    parent("freeze", "release"), parent("regression", "release"), parent("publish", "release"), parent("announce", "release"),
    parent("docs", "release"), parent("docs-api", "docs"), parent("docs-api-examples", "docs-api"),
    parent("scorecards", "vendor"),
    parent("old-1", "old-goal"), parent("old-2", "old-goal"),
    // A second parent in another goal.
    parent("announce", "vendor"),
    blocks("freeze", "regression"), blocks("regression", "publish"), blocks("publish", "announce"),
    { kind: "supersedes", from_id: "docs-api", to_id: "loose" },
  ];
  const attention = [{ binding_id: "b-vendor", work_ref: { item_id: "vendor" }, mode: "pursue", status: { state: "paused" } }];
  return { items, edges, attention };
}

test("goals become frames: roots with children or a binding, nested goals to depth 2, the rest in Other work", () => {
  const model = buildWorkGraphModel({ ...releaseFixture(), readyItemIds: [], now: NOW });
  assert.deepEqual(model.frames.map((frame) => [frame.id, frame.parentFrameId, frame.state]), [
    ["release", null, "in_progress"],
    ["docs", "release", "open"],
    ["vendor", null, "in_progress"],
    [WORKGRAPH_OTHER_FRAME_ID, null, "open"],
    ["old-goal", null, "done"],
  ]);
  const frameOf = (id: string) => model.items.get(id)!.frameId;
  assert.equal(frameOf("release"), "release", "a goal heads its own frame");
  assert.equal(frameOf("regression"), "release");
  assert.equal(frameOf("docs"), "docs", "a goal inside a goal heads a nested frame");
  assert.equal(frameOf("docs-api"), "docs");
  assert.equal(frameOf("docs-api-examples"), "docs", "deeper items join the depth-2 frame");
  assert.equal(model.items.get("docs-api-examples")!.breadcrumb, "docs-api");
  assert.equal(model.items.get("docs-api")!.breadcrumb, null);
  assert.equal(frameOf("scorecards"), "vendor");
  assert.equal(frameOf("loose"), WORKGRAPH_OTHER_FRAME_ID);
  assert.equal(model.items.get("docs-api-examples")!.goalId, "release");
  assert.equal(model.items.get("vendor")!.isGoal, true, "a root with a binding is a goal");

  const release = model.frames.find((frame) => frame.id === "release")!;
  assert.deepEqual(release.itemIds, ["freeze", "regression", "publish", "announce"]);
  assert.deepEqual(release.progress, { total: 7, completed: 1, inProgress: 1, blocked: 1, failed: 0, cancelled: 0, open: 4 });
});

test("without goals there are no frames; a cancelled item does not count toward progress", () => {
  const plain = buildWorkGraphModel({ items: [item("a", at(0)), item("b", at(1))], edges: [], now: NOW });
  assert.deepEqual(plain.frames, []);
  assert.equal(plain.items.get("a")!.frameId, null);

  const goal = buildWorkGraphModel({
    items: [item("g", at(0)), item("c1", at(1), { status: "cancelled" }), item("c2", at(2), { status: "completed" })],
    edges: [parent("c1", "g"), parent("c2", "g")],
    now: NOW,
  });
  assert.deepEqual(goal.frames[0].progress, { total: 1, completed: 1, inProgress: 0, blocked: 0, failed: 0, cancelled: 1, open: 0 });
});

test("parent cycles place every item once", () => {
  const model = buildWorkGraphModel({
    items: [item("a", at(0)), item("b", at(1)), item("c", at(2))],
    edges: [parent("a", "b"), parent("b", "a"), parent("c", "c")],
    now: NOW,
  });
  assert.equal(model.items.size, 3);
  const roots = [...model.items.values()].filter((entry) => entry.parentId === null).map((entry) => entry.id);
  assert.deepEqual(roots, ["b", "c"]);
  assert.equal(model.items.get("a")!.parentId, "b");
});

test("links: blocks and the related family, and a second parent only outside the child's frames", () => {
  const model = buildWorkGraphModel({ ...releaseFixture(), now: NOW });
  assert.deepEqual(model.links.map((link) => [link.relation, link.kind, link.from, link.to]), [
    ["blocks", "blocks", "freeze", "regression"],
    ["blocks", "blocks", "regression", "publish"],
    ["blocks", "blocks", "publish", "announce"],
    ["related", "supersedes", "docs-api", "loose"],
    ["parent", "parent", "announce", "vendor"],
  ]);

  const inside = buildWorkGraphModel({
    items: [item("g", at(0)), item("sub", at(1)), item("leaf", at(2)), item("x", at(3))],
    edges: [parent("sub", "g"), parent("leaf", "sub"), parent("x", "sub"), parent("x", "g")],
    now: NOW,
  });
  assert.deepEqual(inside.links, [], "a second parent whose frame already encloses the child is not drawn again");
  const outside = buildWorkGraphModel({
    items: [item("g", at(0)), item("sub", at(1)), item("leaf", at(2)), item("x", at(3))],
    edges: [parent("sub", "g"), parent("leaf", "sub"), parent("x", "g"), parent("x", "sub")],
    now: NOW,
  });
  assert.deepEqual(outside.links.map((link) => link.id), ["parent:x:sub"], "a child outside its second parent's nested frame links to it");
});

// ── The graph ────────────────────────────────────────────────────────────

test("graph nodes keep the pinned node contract", () => {
  const model = buildWorkGraphModel({ ...releaseFixture(), readyItemIds: [], now: NOW });
  const graph = buildWorkGraphGraph(model, { selectedId: "publish", now: NOW, highlightCriticalPath: true });
  const node = (id: string) => graph.nodes.find((entry) => entry.id === id)!;
  assert.equal(node("release").kind, "goal");
  assert.equal(node("publish").kind, "item");
  assert.equal(node("publish").className, "is-blocked is-selected is-critical");
  assert.deepEqual(node("publish").attributes, { "data-item-id": "publish", "data-status": "blocked", "data-goal-id": "release" });
  assert.equal(node("publish").ariaLabel, "publish, Blocked");
  assert.deepEqual(node("docs").attributes, { "data-item-id": "docs", "data-status": "open", "data-goal-id": "release" });
  assert.equal(node("docs").frame, "release", "a nested goal header sits in its parent frame");
  assert.equal(node("release").frame, undefined);
  assert.equal(node("loose").attributes?.["data-goal-id"], undefined);
  assert.equal(node("vendor").data?.bound, true, "the bound goal carries the goal ring");
  assert.equal(node("regression").avatar, "R");
  assert.deepEqual(graph.accentPath, {
    nodes: ["regression", "publish", "announce"],
    edges: ["blocks:regression:publish", "blocks:publish:announce"],
  });
  assert.equal(buildWorkGraphGraph(model, { now: NOW }).accentPath, null, "the highlight is opt-in");
});

test("graph edges carry their kind and words; done goals fold and cancelled items hide by default", () => {
  const fixture = releaseFixture();
  fixture.items.push(item("dropped", at(14), { status: "cancelled" }));
  fixture.edges.push(parent("dropped", "vendor"));
  const model = buildWorkGraphModel({ ...fixture, now: NOW });
  const graph = buildWorkGraphGraph(model, { now: NOW });
  assert.deepEqual(graph.edges.map((edge) => [edge.id, edge.label, edge.attributes?.["data-kind"]]), [
    ["blocks:freeze:regression", "blocks", "blocks"],
    ["blocks:regression:publish", "blocks", "blocks"],
    ["blocks:publish:announce", "blocks", "blocks"],
    ["supersedes:docs-api:loose", "replaces", "supersedes"],
    ["parent:announce:vendor", "also under", "parent"],
  ]);
  assert.equal(graph.edgeLabels, "always");
  const oldGoal = graph.frames.find((frame) => frame.id === "old-goal")!;
  assert.equal(oldGoal.collapsed, true);
  assert.equal(oldGoal.meta, "2 of 2 done · collapsed");
  assert.equal(graph.hiddenCancelled, 1);
  assert.ok(!graph.nodes.some((node) => node.id === "dropped"));
  assert.equal(graph.notDrawn, 0, "folded and option-hidden items are not 'not drawn'");

  const open = buildWorkGraphGraph(model, { now: NOW, showDone: true, showCancelled: true });
  assert.equal(open.frames.find((frame) => frame.id === "old-goal")!.collapsed, false);
  assert.equal(open.frames.find((frame) => frame.id === "old-goal")!.meta, "2 of 2 done");
  assert.ok(open.nodes.some((node) => node.id === "dropped"));
  assert.equal(open.hiddenCancelled, 0);
});

test("card meta says priority, what holds the item, who has it, where it sits and its age", () => {
  const model = buildWorkGraphModel({ ...releaseFixture(), readyItemIds: ["announce"], now: NOW });
  const meta = (id: string, doneBy?: Map<string, string>) =>
    workGraphCardMeta(model.items.get(id)!, model, { now: NOW, doneBy });
  assert.equal(meta("publish"), "High · Blocked by regression · Unassigned · 3h");
  assert.equal(meta("regression"), "High · Research analyst · 3h");
  assert.equal(meta("freeze"), "Done · 3h");
  assert.equal(meta("freeze", new Map([["freeze", "Release Manager"]])), "Done by Release Manager · 3h");
  assert.equal(meta("docs-api-examples"), "Unassigned · Under docs-api · 3h");
  // A blocked-by item is never also "Ready", whatever the server says.
  assert.equal(meta("announce"), "Low · Blocked by publish · Unassigned · 3h");
});

test("very large graphs draw only what decides the finish, and say how much they left out", () => {
  const items: WorkGraphWireItem[] = [item("goal", at(0), { title: "Big goal" })];
  const edges: WorkGraphWireEdge[] = [];
  for (let index = 0; index < WORKGRAPH_LARGE_ITEM_COUNT + 20; index += 1) {
    const status = index % 10 === 0 ? "in_progress" : index % 10 === 1 ? "blocked" : "open";
    items.push(item(`i${index}`, at(index + 1), { status }));
    edges.push(parent(`i${index}`, "goal"));
  }
  edges.push(blocks("i2", "i3"), blocks("i3", "i4"), blocks("i4", "i5"));
  items.push(item("done-goal", at(900), { status: "completed" }), item("done-child", at(901), { status: "completed" }));
  edges.push(parent("done-child", "done-goal"));
  const model = buildWorkGraphModel({ items, edges, now: NOW });
  const graph = buildWorkGraphGraph(model, { now: NOW, showDone: true });
  assert.equal(graph.lod, true);
  const drawn = graph.nodes.filter((node) => node.kind === "item").map((node) => node.id);
  const expected = model.order.filter((id) => {
    const entry = model.items.get(id)!;
    return id.startsWith("i") && (entry.critical || entry.status === "in_progress" || entry.status === "blocked");
  });
  assert.deepEqual(drawn.filter((id) => id !== "done-child").sort(), expected.sort());
  assert.ok(drawn.includes("i3") && drawn.includes("i5"), "the critical path stays drawn");
  assert.equal(graph.frames.find((frame) => frame.id === "done-goal")!.collapsed, true, "done goals fold even when shown");
  const goalFrame = graph.frames.find((frame) => frame.id === "goal")!;
  const hidden = WORKGRAPH_LARGE_ITEM_COUNT + 20 - expected.length;
  assert.equal(goalFrame.meta, `0 of ${WORKGRAPH_LARGE_ITEM_COUNT + 20} done · +${hidden} items`);
  assert.equal(graph.notDrawn, hidden);

  // The selected item is drawn whatever the level of detail.
  const selected = buildWorkGraphGraph(model, { now: NOW, selectedId: "i7" });
  assert.ok(selected.nodes.some((node) => node.id === "i7"));
});

test("the node cap keeps frame headers, the selection and frame order, and counts the rest", () => {
  const items = Array.from({ length: 250 }, (_, index) => item(`p${String(index).padStart(3, "0")}`, at(index)));
  const model = buildWorkGraphModel({ items, edges: [], now: NOW });
  const graph = buildWorkGraphGraph(model, { now: NOW, cap: 200, selectedId: "p249" });
  assert.equal(graph.nodes.length, 201);
  assert.equal(graph.notDrawn, 49);
  assert.equal(graph.nodes[0].id, "p000");
  assert.ok(graph.nodes.some((node) => node.id === "p249"));
  assert.equal(WORKGRAPH_GRAPH_NODE_CAP, 300);
});

test("undrawable input rows are counted, never silently dropped", () => {
  const model = buildWorkGraphModel({
    items: [item("a", at(0)), { title: "no id", created_at: at(1) }, item("a", at(2), { title: "duplicate id" })],
    edges: [],
    now: NOW,
  });
  assert.equal(model.items.size, 1);
  assert.equal(model.invalidCount, 2);
  assert.equal(buildWorkGraphGraph(model, { now: NOW }).notDrawn, 2);
});

test("more than 40 labelled edges show their words on hover only", () => {
  const items = Array.from({ length: 42 }, (_, index) => item(`e${index}`, at(index)));
  const edges = items.slice(1).map((entry, index) => blocks(items[index].id!, entry.id!));
  const graph = buildWorkGraphGraph(buildWorkGraphModel({ items, edges, now: NOW }), { now: NOW });
  assert.equal(graph.edges.length, 41);
  assert.equal(graph.edgeLabels, "hover");
});

// ── Views, figures and activity ──────────────────────────────────────────

test("the default view: Graph from 720 px and 3 items, Outline otherwise, unmeasured or very large", () => {
  assert.equal(workGraphDefaultView(null, 50), "tree");
  assert.equal(workGraphDefaultView(0, 50), "tree");
  assert.equal(workGraphDefaultView(719, 50), "tree");
  assert.equal(workGraphDefaultView(720, 3), "graph");
  assert.equal(workGraphDefaultView(1020, 2), "tree");
  assert.equal(workGraphDefaultView(1020, WORKGRAPH_LARGE_ITEM_COUNT - 1), "graph");
  assert.equal(workGraphDefaultView(1020, WORKGRAPH_LARGE_ITEM_COUNT), "tree");
});

test("the blocked figure names the one blocker the blocked items wait on", () => {
  const one = buildWorkGraphModel({ ...releaseFixture(), now: NOW });
  assert.equal(workGraphBlockedFoot(one), "waiting on regression");
  const two = buildWorkGraphModel({
    items: [item("a", at(0), { status: "blocked" }), item("b", at(1), { status: "blocked" }), item("x", at(2)), item("y", at(3))],
    edges: [blocks("x", "a"), blocks("y", "b")],
    now: NOW,
  });
  assert.equal(workGraphBlockedFoot(two), "waiting on 2 items");
  const marked = buildWorkGraphModel({ items: [item("a", at(0), { status: "blocked" })], edges: [], now: NOW });
  assert.equal(workGraphBlockedFoot(marked), "marked blocked");
});

test("counts: statuses, ready and the distinct owners of work in progress", () => {
  const model = buildWorkGraphModel({
    items: [
      item("a", at(0), { status: "in_progress", claim: { owner: { key: { kind: "agent", id: "billing" } } } }),
      item("b", at(1), { status: "in_progress", owner: { key: { kind: "agent", id: "billing" } } }),
      item("c", at(2), { status: "in_progress", owner: { key: { kind: "agent", id: "shipping" } } }),
      item("d", at(3), { status: "completed" }),
      item("e", at(4), { status: "failed" }),
      item("f", at(5)),
    ],
    edges: [],
    readyItemIds: ["f"],
    now: NOW,
  });
  assert.deepEqual(model.counts, {
    total: 6, open: 1, inProgress: 3, blocked: 0, completed: 1, cancelled: 0, failed: 1, ready: 1, inProgressOwners: 2,
  });
});

test("activity counts created and closed items per local day, ending today", () => {
  const now = new Date(2026, 6, 8, 15, 0, 0).getTime();
  const day = (offset: number, hour: number) => new Date(2026, 6, 8 - offset, hour, 0, 0).toISOString();
  const days = workGraphActivityDays([
    { kind: "created", at: day(0, 9) },
    { kind: "item_created", at: day(0, 10) },
    { kind: "closed", at: day(0, 11) },
    { kind: "created", at: day(3, 9) },
    { kind: "claimed", at: day(0, 9) },
    { kind: "closed", at: day(9, 9) },
    { kind: "created", at: "not a time" },
  ], now);
  assert.equal(days.length, 7);
  assert.deepEqual(days.map((entry) => [entry.created, entry.closed]), [[0, 0], [0, 0], [0, 0], [1, 0], [0, 0], [0, 0], [2, 1]]);
  assert.equal(days[6].start, new Date(2026, 6, 8).getTime());
});

test("done by: the owner who held the item when it closed, else plain Done", () => {
  const claim = (seq: number, itemId: string, id: string, name?: string) => ({
    seq, kind: "claimed", item_id: itemId, at: at(seq),
    payload: { item: { id: itemId, claim: { owner: { key: { kind: "agent", id }, display_name: name } } } },
  });
  const closed = (seq: number, itemId: string, status = "completed") => ({
    seq, kind: "closed", item_id: itemId, at: at(seq), payload: { item: { id: itemId, status } },
  });
  const released = (seq: number, itemId: string) => ({ seq, kind: "released", item_id: itemId, at: at(seq) });
  const doneBy = workGraphDoneBy([
    // Out of order on purpose: the window is read by seq.
    claim(3, "x", "billing", "Billing"), closed(4, "x"), claim(1, "x", "returns", "Returns"),
    claim(2, "open-one", "shipping"),
    claim(5, "dropped", "intake"), released(6, "dropped"), closed(7, "dropped"),
    claim(8, "broke", "orders"), closed(9, "broke", "failed"),
    closed(10, "no-claim"),
  ]);
  assert.deepEqual([...doneBy], [["x", "Billing"]]);
});

test("ages read compactly", () => {
  assert.equal(workGraphAge(undefined, NOW), "");
  assert.equal(workGraphAge("not a time", NOW), "");
  assert.equal(workGraphAge("2026-07-08T11:59:40Z", NOW), "now");
  assert.equal(workGraphAge("2026-07-08T11:48:00Z", NOW), "12m");
  assert.equal(workGraphAge("2026-07-08T07:00:00Z", NOW), "5h");
  assert.equal(workGraphAge("2026-07-05T12:00:00Z", NOW), "3d");
  assert.equal(workGraphAge("2026-06-01T12:00:00Z", NOW), "5w");
});

test("workgraph links and statuses read in words, never as raw kinds", () => {
  assert.equal(workGraphEdgeWord("blocks"), "blocks");
  assert.equal(workGraphEdgeWord("related"), "related");
  assert.equal(workGraphEdgeWord("supersedes"), "replaces");
  assert.equal(workGraphEdgeWord("derived_from"), "derived from");
  assert.equal(workGraphEdgeWord("parent"), "also under");
  assert.equal(workGraphEdgeWord("duplicate_of"), "duplicate of");
  assert.equal(workGraphStatusWord("in_progress"), "In progress");
  assert.equal(workGraphStatusWord(undefined), "Open");
});
