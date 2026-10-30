import assert from "node:assert/strict";
import test from "node:test";

import React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import { WorkGraphPanel, __workGraphPanelTest } from "./WorkGraphPanel";
import { WorkGraphGraphView, __workGraphGraphViewTest } from "./WorkGraphGraphView";
import type { WorkGraphPanelData } from "./WorkGraphPanel";
import type { WorkGraphWireBinding, WorkGraphWireEdge, WorkGraphWireItem } from "../types";
import { workGraphItemOwner, workGraphStatusWord } from "../lib/workgraph-layout";
import { createAgentDirectory } from "../../../packages/console-components/src/panel-kit/agents";
import { formatClockTime } from "../../../packages/console-components/src/panel-kit/format";

function localDateTime(instant: string): string {
  return new Intl.DateTimeFormat("sv-SE", { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(new Date(instant));
}

const {
  buildWorkGraphPanelTree,
  workGraphBindingStatusLabel,
  workGraphBindingTargetLabel,
  workGraphEventLine,
  workGraphOwnerLabelOf,
  workGraphGoalRevisionOf,
  workGraphEventsParams,
  workGraphEventsNewestFirst,
  createWorkGraphRefreshSequencer,
  workGraphModeWord,
  workGraphBindingStateText,
  workGraphBindingTarget,
  workGraphSummary,
  groupWorkGraphTree,
  workGraphBlockersOf,
  workGraphEventSentence,
} = __workGraphPanelTest;

function item(id: string, createdAt: string, extra: Partial<WorkGraphWireItem> = {}): WorkGraphWireItem {
  return { id, title: id, created_at: createdAt, status: "open", revision: 1, ...extra };
}

test("workgraph panel tree orders roots by creation and indents children under parents", () => {
  const rows = buildWorkGraphPanelTree(
    [
      item("goal-b", "2026-07-08T09:00:00Z"),
      item("goal-a", "2026-07-08T08:00:00Z"),
      item("child-1", "2026-07-08T08:10:00Z"),
      item("grandchild-1", "2026-07-08T08:20:00Z"),
    ],
    [
      // Parent edges run child→parent.
      { kind: "parent", from_id: "child-1", to_id: "goal-a" },
      { kind: "parent", from_id: "grandchild-1", to_id: "child-1" },
      // Non-parent edges never shape the tree.
      { kind: "blocks", from_id: "goal-b", to_id: "goal-a" },
    ],
  );

  assert.deepEqual(
    rows.map((row) => [row.itemId, row.depth]),
    [
      ["goal-a", 0],
      ["child-1", 1],
      ["grandchild-1", 2],
      ["goal-b", 0],
    ],
  );
});

test("workgraph panel tree treats children of unknown parents as roots", () => {
  const rows = buildWorkGraphPanelTree(
    [item("orphan", "2026-07-08T08:00:00Z")],
    [{ kind: "parent", from_id: "orphan", to_id: "missing-parent" }],
  );
  assert.deepEqual(rows.map((row) => [row.itemId, row.depth]), [["orphan", 0]]);
});

test("workgraph panel tree ignores self-parent cycles", () => {
  const rows = buildWorkGraphPanelTree(
    [item("a", "2026-07-08T08:00:00Z"), item("b", "2026-07-08T08:05:00Z")],
    [
      { kind: "parent", from_id: "a", to_id: "a" },
      { kind: "parent", from_id: "b", to_id: "a" },
    ],
  );
  assert.deepEqual(rows.map((row) => [row.itemId, row.depth]), [["a", 0], ["b", 1]]);
});

test("workgraph binding status labels cover active, paused-with-deadline, and terminal states", () => {
  const binding = (status: WorkGraphWireBinding["status"]): WorkGraphWireBinding => ({
    binding_id: "attention-1",
    status,
  });
  assert.equal(workGraphBindingStatusLabel(binding({ state: "active" })), "active");
  assert.equal(workGraphBindingStatusLabel(binding(undefined)), "active");
  assert.equal(workGraphBindingStatusLabel(binding({ state: "paused" })), "paused");
  assert.equal(
    workGraphBindingStatusLabel(binding({ state: "paused", until: "2026-07-09T10:30:00Z" })),
    `paused until ${localDateTime("2026-07-09T10:30:00Z")}`,
  );
  assert.equal(workGraphBindingStatusLabel(binding({ state: "superseded" })), "superseded");
  assert.equal(workGraphBindingStatusLabel(binding({ state: "stopped" })), "stopped");
});

test("workgraph binding target labels cover session and lowered-owner targets", () => {
  assert.equal(
    workGraphBindingTargetLabel({ target: { kind: "session", session_id: "sess-42" } }),
    "sess-42",
  );
  assert.equal(
    workGraphBindingTargetLabel({
      target: { kind: "lowered_owner", owner_key: { kind: "agent", id: "planner" } },
    }),
    "agent:planner",
  );
  assert.equal(workGraphBindingTargetLabel({}), "");
});

test("workgraph event lines render timestamp, kind, and item id compactly", () => {
  assert.equal(
    workGraphEventLine({ kind: "item_claimed", at: "2026-07-08T09:15:30Z", item_id: "item-1" }),
    `${localDateTime("2026-07-08T09:15:30Z")} · item claimed · item-1`,
  );
  assert.equal(workGraphEventLine({}), "event");
});

test("workgraph goal revision resolves the binding's bound work item, not the binding machine", () => {
  const items: WorkGraphWireItem[] = [
    item("goal-1", "2026-07-08T08:00:00Z", { revision: 4 }),
    item("child-1", "2026-07-08T08:10:00Z", { revision: 2 }),
  ];
  const binding: WorkGraphWireBinding = {
    binding_id: "attention-1",
    work_ref: { item_id: "goal-1" },
    machine_state: { revision: 7 },
  };
  assert.equal(workGraphGoalRevisionOf(binding, items), 4);
  // Unknown bound item / missing work_ref: no token (the action falls back
  // to 0 and surfaces the CAS conflict rather than silently guessing).
  assert.equal(
    workGraphGoalRevisionOf({ binding_id: "b", work_ref: { item_id: "gone" } }, items),
    undefined,
  );
  assert.equal(workGraphGoalRevisionOf({ binding_id: "b" }, items), undefined);
});

test("workgraph events params page from the snapshot high-water mark so the tail never freezes", () => {
  // Upstream returns ASCENDING truncated to limit: a bare {limit} query pins
  // the oldest window once the ledger outgrows it.
  assert.deepEqual(workGraphEventsParams(137, 50), { limit: 50, after_seq: 87 });
  assert.deepEqual(workGraphEventsParams(50, 50), { limit: 50, after_seq: 0 });
  assert.deepEqual(workGraphEventsParams(12, 50), { limit: 50, after_seq: 0 });
  // Fresh store (null mark) and older runtimes (absent mark) fall back to
  // the bare query.
  assert.deepEqual(workGraphEventsParams(null, 50), { limit: 50 });
  assert.deepEqual(workGraphEventsParams(undefined, 50), { limit: 50 });
});

test("workgraph events render newest-first without mutating the wire order", () => {
  const ascending = [{ seq: 1, kind: "a" }, { seq: 2, kind: "b" }, { seq: 3, kind: "c" }];
  const rendered = workGraphEventsNewestFirst(ascending);
  assert.deepEqual(rendered.map((event) => event.seq), [3, 2, 1]);
  assert.deepEqual(ascending.map((event) => event.seq), [1, 2, 3]);
  assert.deepEqual(workGraphEventsNewestFirst([]), []);
});

test("workgraph refresh sequencer invalidates stale refreshes the moment a newer one begins", () => {
  const sequencer = createWorkGraphRefreshSequencer();
  const first = sequencer.begin();
  assert.equal(first(), true);
  const second = sequencer.begin();
  assert.equal(first(), false, "an older refresh must not overwrite a newer one");
  assert.equal(second(), true);
  const third = sequencer.begin();
  assert.equal(second(), false);
  assert.equal(third(), true);
});

// ── Graph view rendering ─────────────────────────────────────────────────

function graphFixture(): { items: WorkGraphWireItem[]; edges: WorkGraphWireEdge[] } {
  return {
    items: [
      item("root", "2026-07-08T08:00:00Z", { title: "Ship it" }),
      item("child-run", "2026-07-08T08:10:00Z", {
        status: "in_progress",
        claim: { owner: { key: { kind: "agent", id: "helper" } } },
      }),
      item("child-done", "2026-07-08T08:20:00Z", { status: "completed" }),
      item("child-stuck", "2026-07-08T08:30:00Z", { status: "blocked" }),
    ],
    edges: [
      { kind: "parent", from_id: "child-run", to_id: "root" },
      { kind: "parent", from_id: "child-done", to_id: "root" },
      { kind: "parent", from_id: "child-stuck", to_id: "root" },
      { kind: "blocks", from_id: "child-run", to_id: "child-stuck" },
    ],
  };
}

function panelData(overrides: Partial<WorkGraphPanelData> = {}): WorkGraphPanelData {
  const fixture = graphFixture();
  return {
    items: fixture.items,
    edges: fixture.edges,
    attention: [],
    events: [],
    capturedAt: "2026-07-08T09:00:00Z",
    unavailable: false,
    denied: false,
    error: null,
    ...overrides,
  };
}

function count(html: string, needle: string): number {
  return html.split(needle).length - 1;
}

test("workgraph panel head renders the tree/graph toggle and defaults to the tree", () => {
  const html = renderToStaticMarkup(
    React.createElement(WorkGraphPanel, {
      data: panelData(),
      canManage: false,
      onRefresh: () => {},
    }),
  );
  assert.ok(html.includes('data-testid="workgraph-view-toggle:tree"'));
  assert.ok(html.includes('data-testid="workgraph-view-toggle:graph"'));
  // Default mode is the tree: item rows render, the graph svg does not.
  assert.ok(html.includes('data-testid="workgraph-panel-item:root"'));
  assert.ok(!html.includes('data-testid="workgraph-graph"'));
  // The existing head affordances stay untouched.
  assert.ok(html.includes('data-testid="workgraph-panel-refresh"'));
});

test("workgraph graph view draws nodes with status classes and typed edges", () => {
  const fixture = graphFixture();
  const html = renderToStaticMarkup(
    React.createElement(WorkGraphGraphView, {
      items: fixture.items,
      edges: fixture.edges,
      attention: [{ binding_id: "b-1", work_ref: { item_id: "root" } }],
      selectedId: "child-run",
    }),
  );
  assert.equal(count(html, 'data-testid="workgraph-graph-node"'), 4);
  assert.equal(count(html, 'data-testid="workgraph-graph-edge"'), 4);
  assert.ok(html.includes("is-in_progress"));
  assert.ok(html.includes("is-completed"));
  assert.ok(html.includes("is-blocked"));
  assert.ok(html.includes('data-kind="blocks"'));
  assert.ok(html.includes("is-selected"));
  assert.ok(html.includes('data-testid="workgraph-graph-viewport"'));
  assert.ok(html.includes('data-testid="workgraph-graph-fit"'));
  // The attention-bound root carries the goal ring.
  assert.ok(html.includes("workgraph-graph__node-goal-ring"));
  // Selection detail footer names the selected item.
  assert.ok(html.includes('data-testid="workgraph-graph-detail"'));
  assert.ok(html.includes("child-run"));
});

test("selected work item exposes its full title and description before collapsed exact identifiers", () => {
  const selected = item("work_01a0daba-4ca1-7073-8471-02f7df3d923e", "2026-07-08T08:00:00Z", {
    title: "Publish the release candidate after both independent reviews have completed",
    description: "Verify the source review and release badge.\nPreserve both reviewers' evidence before publishing the report.",
    status: "in_progress",
    owner: { display_name: "Release coordinator" },
    labels: ["release:2026-09", "evidence:required"],
  });
  const html = renderToStaticMarkup(React.createElement(WorkGraphGraphView, {
    items: [selected], edges: [], attention: [], selectedId: selected.id,
  }));
  const detail = html.slice(html.indexOf('data-testid="workgraph-graph-detail"'));
  assert.ok(detail.includes(`<h4 class="workgraph-graph__detail-title">${selected.title}</h4>`), "selection has a full heading, independent of truncated node text");
  assert.ok(detail.includes("Verify the source review and release badge.\nPreserve both reviewers&#x27; evidence before publishing the report."), "description keeps every source line");
  assert.ok(detail.includes("In progress"));
  assert.ok(detail.includes("Release coordinator"));
  const disclosureStart = detail.indexOf("<details");
  assert.ok(disclosureStart > detail.indexOf("workgraph-graph__detail-description"), "identifiers remain secondary to readable description");
  const disclosure = detail.slice(disclosureStart);
  assert.match(disclosure, /^<details[^>]*>/);
  assert.doesNotMatch(disclosure.match(/^<details[^>]*>/)![0], /\bopen(?:=|\s|>)/, "metadata starts collapsed");
  assert.ok(disclosure.includes(`<code>${selected.id}</code>`), "exact canonical id remains selectable");
  assert.ok(disclosure.includes('aria-label="Copy work item ID"'), "exact id has a discoverable copy action");
  assert.ok(disclosure.includes("release:2026-09") && disclosure.includes("evidence:required"));
});

test("workgraph tree view caps rendered rows with the graph's overflow honesty", () => {
  // The snapshot includes terminal rows, so it tracks the store's full
  // history; the tree must stay render-bounded like the graph is.
  const items = Array.from({ length: 205 }, (_, index) =>
    item(`item-${String(index).padStart(3, "0")}`, "2026-07-08T08:00:00Z"));
  const html = renderToStaticMarkup(
    React.createElement(WorkGraphPanel, {
      data: panelData({ items, edges: [] }),
      canManage: false,
      onRefresh: () => {},
    }),
  );
  assert.equal(count(html, 'data-testid="workgraph-panel-item:'), 200);
  assert.ok(html.includes('data-testid="workgraph-panel-overflow"'));
  assert.ok(html.includes("+5 more items not shown"));
});

test("workgraph graph view reports overflow past the node cap", () => {
  const items = Array.from({ length: 205 }, (_, index) =>
    item(`item-${String(index).padStart(3, "0")}`, "2026-07-08T08:00:00Z"));
  const html = renderToStaticMarkup(
    React.createElement(WorkGraphGraphView, { items, edges: [], attention: [] }),
  );
  assert.equal(count(html, 'data-testid="workgraph-graph-node"'), 200);
  assert.ok(html.includes('data-testid="workgraph-graph-overflow"'));
  assert.ok(html.includes("+5 more items not drawn"));
});

test("workgraph owner labels prefer display names, then key ids, then claim owners", () => {
  assert.equal(
    workGraphOwnerLabelOf({ owner: { key: { kind: "agent", id: "planner" }, display_name: "Planner" } }),
    "Planner",
  );
  assert.equal(
    workGraphOwnerLabelOf({ owner: { key: { kind: "agent", id: "planner" } } }),
    "planner",
  );
  assert.equal(
    workGraphOwnerLabelOf({ claim: { owner: { key: { kind: "session", id: "sess-42" } } } }),
    "sess-42",
  );
  assert.equal(workGraphOwnerLabelOf({}), "");
});

test("workgraph graph fit never enlarges and never shrinks below the legibility floor", () => {
  const { fitViewport, FIT_MIN_SCALE } = __workGraphGraphViewTest;
  // Small graph: 1:1, centred in the frame.
  assert.deepEqual(fitViewport(800, 380, 400, 200), { tx: 200, ty: 90, scale: 1 });
  // Slightly too big: scaled down to fit, centred on the slack axis.
  const snug = fitViewport(800, 380, 880, 200);
  assert.ok(Math.abs(snug.scale - 800 / 880) < 1e-9);
  assert.equal(snug.tx, 0);
  assert.ok(snug.ty > 0);
  // Much too big: floored, pinned top-left so the user pans from the start.
  const tall = fitViewport(800, 380, 488, 1400);
  assert.equal(tall.scale, FIT_MIN_SCALE);
  assert.equal(tall.ty, 0);
  assert.ok(tall.tx > 0, "narrow axis still centres");
  // Unmeasured frame falls back to identity.
  assert.deepEqual(fitViewport(0, 0, 400, 200), { tx: 0, ty: 0, scale: 1 });
});

test("workgraph graph labels truncate by measured width with a character fallback", () => {
  const { fitLabel } = __workGraphGraphViewTest;
  const mono: (text: string) => number = (text) => text.length * 7;
  assert.equal(fitLabel("Docs", 132, mono, 21), "Docs");
  const long = "Make the semver readiness gate declare breaking changes";
  const fitted = fitLabel(long, 132, mono, 21);
  assert.ok(fitted.endsWith("…"));
  assert.ok(mono(fitted) <= 132, `fits: ${fitted}`);
  // No trailing space before the ellipsis.
  assert.ok(!/\s…$/.test(fitted));
  // A wider face (the terminal variant's monospace) keeps fewer characters.
  const wide: (text: string) => number = (text) => text.length * 8;
  assert.ok(fitLabel(long, 132, wide, 21).length < fitted.length);
  // Without a measurer (server render) the character cap applies.
  assert.equal(fitLabel(long, 132, null, 21), `${long.slice(0, 20)}…`);
});


test("workgraph local timestamps retain the local date across midnight", () => {
  const previousTimeZone = process.env.TZ;
  process.env.TZ = "America/Los_Angeles";
  try {
    const at = "2026-07-09T01:15:30Z";
    assert.equal(workGraphEventLine({ kind: "item_claimed", at, item_id: "item-1" }), "2026-07-08 18:15 · item claimed · item-1");
    assert.equal(workGraphBindingStatusLabel({ status: { state: "paused", until: at } }), "paused until 2026-07-08 18:15");
    const html = renderToStaticMarkup(React.createElement(WorkGraphPanel, { data: panelData({ capturedAt: at }), canManage: false, onRefresh: () => {} }));
    assert.match(html, /as of 2026-07-08 18:15:30/);
  } finally {
    if (previousTimeZone === undefined) delete process.env.TZ;
    else process.env.TZ = previousTimeZone;
  }
});

test("workgraph local timestamps omit invalid event pause and snapshot times", () => {
  const at = "not-a-valid-time-but-long-enough";
  assert.equal(workGraphEventLine({ kind: "item_claimed", at, item_id: "item-1" }), "item claimed · item-1");
  assert.equal(workGraphBindingStatusLabel({ status: { state: "paused", until: at } }), "paused");
  const html = renderToStaticMarkup(React.createElement(WorkGraphPanel, { data: panelData({ capturedAt: at }), canManage: false, onRefresh: () => {} }));
  assert.doesNotMatch(html, /as of|workgraph__captured|Invalid Date|not-a-valid-time/);
});

// ── Display: words, names and sentences instead of raw values ───────────

test("workgraph statuses and binding modes read as words, never raw enums", () => {
  assert.equal(workGraphStatusWord("in_progress"), "In progress");
  assert.equal(workGraphStatusWord("completed"), "Completed");
  assert.equal(workGraphStatusWord(undefined), "Open");
  assert.equal(workGraphStatusWord("needs_review"), "Needs review");
  // Modes say what the agent does with the goal, never the mode's name.
  assert.equal(workGraphModeWord("coordinate"), "Leading the work");
  assert.equal(workGraphModeWord("falsify"), "Testing the work");
  assert.equal(workGraphModeWord(undefined), "Doing the work");
  assert.equal(workGraphModeWord("pursue"), "Doing the work");
  assert.equal(workGraphModeWord("review"), "Reviewing the work");
  assert.equal(workGraphModeWord("judge"), "Judging the work");
  assert.equal(workGraphModeWord("observe"), "Watching the work");
  assert.equal(workGraphModeWord("escalate_upward"), "Escalate upward");
  for (const mode of ["coordinate", "pursue", "review", "falsify", "judge", "observe"]) {
    assert.doesNotMatch(workGraphModeWord(mode), /coordinat|pursu|falsif|observ/i, mode);
  }
});

test("workgraph binding states read as words with a nearby pause end", () => {
  const now = Date.parse("2026-07-08T12:00:00Z");
  assert.equal(workGraphBindingStateText({}, now), "Active");
  assert.equal(workGraphBindingStateText({ status: { state: "active" } }, now), "Active");
  assert.equal(workGraphBindingStateText({ status: { state: "superseded" } }, now), "Replaced");
  assert.equal(workGraphBindingStateText({ status: { state: "stopped" } }, now), "Stopped");
  assert.equal(workGraphBindingStateText({ status: { state: "paused" } }, now), "Paused");
  assert.equal(workGraphBindingStateText({ status: { state: "paused", until: "not-a-valid-time" } }, now), "Paused");
  // The same day reads as the clock alone.
  const soon = "2026-07-08T12:30:00Z";
  assert.equal(
    workGraphBindingStateText({ status: { state: "paused", until: soon } }, now),
    `Paused until ${formatClockTime(soon)}`,
  );
  const later = workGraphBindingStateText({ status: { state: "paused", until: "2026-07-10T12:00:00Z" } }, now);
  assert.ok(later.startsWith("Paused until ") && later.endsWith(formatClockTime("2026-07-10T12:00:00Z")), later);
  assert.doesNotMatch(later, /Invalid|2026-07-10T/);
});

test("workgraph names resolve through the agent directory and never show an unknown session id", () => {
  const directory = createAgentDirectory([
    { id: "identity:coordinator", label: "Coordinator", aliases: ["session-77"] },
    { id: "identity:release-manager", label: "Release Manager" },
  ]);
  assert.deepEqual(
    workGraphBindingTarget({ target: { kind: "session", session_id: "session-77" } }, directory),
    { name: "Coordinator", key: "session-77" },
  );
  assert.deepEqual(
    workGraphBindingTarget({ target: { kind: "session", session_id: "session-01JBX4N0Q3T9" } }, directory),
    { name: "Agent session", key: "session-01JBX4N0Q3T9" },
  );
  assert.deepEqual(
    workGraphBindingTarget({ target: { kind: "lowered_owner", owner_key: { kind: "agent", id: "coordinator" } } }, directory),
    { name: "Coordinator", key: "agent:coordinator" },
  );
  assert.deepEqual(
    workGraphBindingTarget({ target: { kind: "lowered_owner", owner_key: { kind: "agent", id: "planner" } } }),
    { name: "planner", key: "agent:planner" },
  );
  assert.equal(workGraphBindingTarget({}), null);

  // Owners: the roster name, then the stored name, then the bare key.
  assert.deepEqual(
    workGraphItemOwner({ owner: { key: { kind: "agent", id: "release-manager" }, display_name: "RM" } }, directory),
    { name: "Release Manager", key: "agent:release-manager" },
  );
  assert.deepEqual(
    workGraphItemOwner({ owner: { key: { kind: "agent", id: "planner" }, display_name: "Planner" } }, directory),
    { name: "Planner", key: "agent:planner" },
  );
  assert.deepEqual(
    workGraphItemOwner({ claim: { owner: { key: { kind: "principal", id: "admin@example.test" } } } }),
    { name: "admin@example.test", key: "principal:admin@example.test" },
  );
  assert.deepEqual(
    workGraphItemOwner({ claim: { owner: { key: { kind: "session", id: "sess-42" } } } }),
    { name: "Agent session", key: "session:sess-42" },
  );
  assert.equal(workGraphItemOwner({}), null);
});

test("workgraph summary counts items with real plurals", () => {
  assert.equal(workGraphSummary([]), "No work items");
  assert.equal(workGraphSummary([item("only", "2026-07-08T08:00:00Z")]), "1 item");
  assert.equal(workGraphSummary(graphFixture().items), "4 items · 1 in progress · 1 blocked");
});

test("workgraph tree groups follow their roots and count progress past the row cap", () => {
  const fixture = graphFixture();
  const rows = buildWorkGraphPanelTree(fixture.items, fixture.edges);
  const [group] = groupWorkGraphTree(rows);
  assert.equal(group.root.itemId, "root");
  assert.deepEqual(group.children.map((row) => row.itemId), ["child-run", "child-done", "child-stuck"]);
  assert.deepEqual(group.progress, { total: 3, completed: 1, inProgress: 1, blocked: 1, failed: 0 });
  const capped = groupWorkGraphTree(rows, 2);
  assert.deepEqual(capped[0].children.map((row) => row.itemId), ["child-run"]);
  assert.equal(capped[0].progress.total, 3, "items past the cap still count toward their goal");
});

test("workgraph blockers name the unfinished items that block an item", () => {
  const fixture = graphFixture();
  assert.deepEqual([...workGraphBlockersOf(fixture.items, fixture.edges)], [["child-stuck", ["child-run"]]]);
  const finished = fixture.items.map((entry) => (entry.id === "child-run" ? { ...entry, status: "completed" } : entry));
  assert.equal(workGraphBlockersOf(finished, fixture.edges).size, 0, "a completed blocker no longer blocks");
});

test("workgraph activity reads as sentences about item titles, without item ids", () => {
  const titles = new Map([["item-1", "Freeze the release branch"]]);
  const context = { titleOf: (id: string) => titles.get(id) };
  assert.equal(workGraphEventSentence({ kind: "created", item_id: "item-1" }, context), "Freeze the release branch was created");
  assert.equal(workGraphEventSentence({ kind: "item_claimed", item_id: "item-1" }, context), "Freeze the release branch was claimed");
  assert.equal(
    workGraphEventSentence({ kind: "claimed", item_id: "item-1", payload: { item: { claim: { owner: { key: { kind: "agent", id: "builder" } } } } } }, context),
    "builder claimed Freeze the release branch",
  );
  assert.equal(
    workGraphEventSentence({ kind: "closed", item_id: "item-1", payload: { item: { status: "completed" } } }, context),
    "Freeze the release branch was completed",
  );
  // Attention events say which agent works toward the goal, never "attention".
  assert.equal(
    workGraphEventSentence({ kind: "attention_updated", payload: { attention: { work_ref: { item_id: "item-1" }, status: { state: "paused" } } } }, context),
    "Freeze the release branch was paused for its agent",
  );
  const directory = createAgentDirectory([{ id: "identity:coordinator", label: "Coordinator" }]);
  const named = { ...context, directory };
  const onGoal = (state?: string) => ({
    work_ref: { item_id: "item-1" },
    target: { kind: "owner", owner_key: { kind: "identity", id: "coordinator" } },
    ...(state ? { status: { state } } : {}),
  });
  assert.equal(
    workGraphEventSentence({ kind: "attention_created", payload: { attention: onGoal() } }, named),
    "Coordinator was put on Freeze the release branch",
  );
  assert.equal(
    workGraphEventSentence({ kind: "attention_paused", payload: { attention: onGoal("paused") } }, named),
    "Freeze the release branch was paused for Coordinator",
  );
  assert.equal(
    workGraphEventSentence({ kind: "attention_updated", payload: { attention: onGoal("stopped") } }, named),
    "Coordinator was taken off Freeze the release branch",
  );
  assert.equal(
    workGraphEventSentence({ kind: "attention_updated", payload: { attention: onGoal("active") } }, named),
    "Coordinator's part in Freeze the release branch changed",
  );
  // A session the roster does not know is "an agent", never its ID.
  assert.equal(
    workGraphEventSentence({
      kind: "attention_created",
      payload: { attention: { work_ref: { item_id: "item-1" }, target: { kind: "session", session_id: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D" } } },
    }, named),
    "An agent was put on Freeze the release branch",
  );
  for (const kind of ["attention_created", "attention_paused", "attention_updated"]) {
    assert.doesNotMatch(
      workGraphEventSentence({ kind, payload: { attention: onGoal() } }, named),
      /attention/i,
      kind,
    );
  }
  assert.equal(
    workGraphEventSentence({ kind: "linked", payload: { edge: { kind: "blocks", from_id: "item-1", to_id: "gone" } } }, context),
    "Freeze the release branch now blocks another work item",
  );
  assert.equal(workGraphEventSentence({ kind: "blocked", item_id: "unknown-id" }, context), "A work item became blocked");
  assert.equal(workGraphEventSentence({}, context), "Activity on a work item");
});

// ── Panel rendering ──────────────────────────────────────────────────────

function renderPanel(data: WorkGraphPanelData, canManage = false): string {
  return renderToStaticMarkup(React.createElement(WorkGraphPanel, {
    data,
    canManage,
    onRefresh: () => {},
    onClaim: () => {},
    onClose: () => {},
    onGoalConfirm: () => {},
    onGoalRequestClose: () => {},
    onAttentionPause: () => {},
    onAttentionResume: () => {},
    onAttentionReassign: () => {},
  }));
}

test("workgraph goals render as cards holding their attention and items; plain items stay rows", () => {
  const fixture = graphFixture();
  const html = renderPanel(panelData({
    items: [...fixture.items, item("loose", "2026-07-08T10:00:00Z", { title: "Rotate the API keys" })],
    attention: [{
      binding_id: "b-1",
      work_ref: { item_id: "root" },
      mode: "coordinate",
      status: { state: "active" },
      target: { kind: "session", session_id: "session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D" },
    }],
    events: [{ seq: 4, kind: "item_created", item_id: "loose", at: "2026-07-08T10:00:00Z" }],
  }), true);
  const card = html.slice(html.indexOf('<article class="workgraph-goal"'), html.indexOf("</article>"));
  assert.ok(card.includes('data-testid="workgraph-panel-item:root"'));
  assert.ok(card.includes("1 of 3 done"), "the goal shows progress over its items");
  assert.ok(card.includes('data-testid="workgraph-panel-binding:b-1"'), "attention sits in its goal's card");
  // The mode reads as what the agent does; the mode's name is the tooltip.
  assert.ok(card.includes('title="Attention mode: coordinate">Leading the work</span>'));
  assert.ok(!/Coordinating|Pursuing/.test(card));
  assert.ok(card.includes('data-testid="workgraph-panel-item:child-run"'));
  // An unknown session reads as words; its id stays in the technical details.
  assert.ok(card.includes("Agent session"));
  const technical = card.slice(card.indexOf('<details class="cc-tech'));
  assert.ok(technical.includes("session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D"));
  assert.ok(!card.slice(0, card.indexOf('<details class="cc-tech')).includes(">session-01JBX4N0Q3T9K2W8Y6ZR5M7C1D<"));
  // The plain item is a row outside any card, with its actions.
  assert.ok(!card.includes("workgraph-panel-item:loose"));
  assert.ok(html.includes('data-testid="workgraph-panel-item:loose"'));
  assert.ok(html.includes('data-testid="workgraph-panel-action:loose:claim"'));
  assert.ok(html.includes('data-testid="workgraph-panel-action:loose:close"'));
  // Claim needs an open, unowned item; Done skips finished and blocked ones.
  assert.ok(html.includes('data-testid="workgraph-panel-action:child-run:close"'));
  assert.ok(!html.includes('data-testid="workgraph-panel-action:child-run:claim"'));
  assert.ok(!html.includes('data-testid="workgraph-panel-action:child-done:close"'));
  assert.ok(!html.includes('data-testid="workgraph-panel-action:child-stuck:close"'));
  assert.ok(html.includes("Blocked by child-run"));
  // In progress is the info tone, never the accent.
  const running = html.slice(html.indexOf('data-testid="workgraph-panel-item:child-run"'));
  assert.match(running, /^[^>]*>\s*<span class="cc-row__lead"><span class="cc-dot" data-tone="info"/);
  // Activity is a sentence about the title, not a kind and an id.
  assert.ok(html.includes("Rotate the API keys was created"));
  assert.ok(!html.includes("item created · loose"));
  // One header (pinned class), one heading for the panel, fresh-as-of in the tooltip.
  assert.equal(count(html, "workgraph__head"), 1);
  assert.match(html, /<h2 class="cc-panel-head__title">WorkGraph<\/h2>/);
  assert.match(html, /Updated [^<]+<\/time>/);
  assert.match(html, /title="Snapshot as of /);
});

test("workgraph denied, unavailable, loading and failed states are designed, with one alert at most", () => {
  const denied = renderPanel(panelData({ denied: true, items: [], edges: [] }));
  assert.ok(denied.includes('data-testid="workgraph-panel-denied"'));
  assert.ok(denied.includes("have access to work items"));
  assert.ok(!denied.includes('role="alert"'));

  const unavailable = renderPanel(panelData({ unavailable: true }));
  assert.ok(unavailable.includes("set up on this runtime"));
  assert.ok(unavailable.includes('data-testid="workgraph-panel"'));

  const loading = renderPanel(panelData({ capturedAt: null, items: [], edges: [] }));
  assert.ok(loading.includes('role="status"'));
  assert.ok(loading.includes("Loading work items"));
  assert.ok(!loading.includes("No work items yet"));

  const failed = renderPanel(panelData({ capturedAt: null, items: [], edges: [], error: "store unavailable" }));
  assert.equal(count(failed, 'role="alert"'), 1);
  assert.ok(failed.includes("could not be loaded"));

  const empty = renderPanel(panelData({ items: [], edges: [] }));
  assert.ok(empty.includes("No work items yet"));

  // A failed refresh keeps the items and shows its single alert in the status row.
  const stale = renderPanel(panelData({ error: "store unavailable" }));
  assert.equal(count(stale, 'role="alert"'), 1);
  assert.ok(stale.includes('data-testid="workgraph-panel-item:root"'));
});

test("workgraph graph nodes are keyboard reachable buttons named by title and status", () => {
  const fixture = graphFixture();
  const html = renderToStaticMarkup(React.createElement(WorkGraphGraphView, {
    items: fixture.items,
    edges: fixture.edges,
    attention: [],
    selectedId: "child-run",
  }));
  assert.equal(count(html, 'role="button"'), 4);
  assert.equal(count(html, 'tabindex="0"'), 4);
  assert.ok(html.includes('aria-label="Ship it, Open"'));
  assert.ok(html.includes('aria-label="child-run, In progress"'));
  assert.ok(html.includes('data-tone="info"'), "in progress nodes take the info tone");
  assert.ok(html.includes("workgraph-graph__node-ring"), "the selected node carries the selection ring");
  assert.ok(!html.includes('role="img"'), "the graph is a group people can enter, not an image");
});

test("workgraph agents on goals and graph links read in plain words", () => {
  // A binding whose item is not in the snapshot lists under the other goals.
  const html = renderPanel(panelData({
    attention: [{
      binding_id: "b-off",
      work_ref: { item_id: "not-in-snapshot" },
      mode: "review",
      status: { state: "superseded" },
    }],
  }));
  assert.ok(html.includes('<span title="Attention bindings">Agents on other goals</span>'));
  assert.ok(!/Other attention|>Attention</.test(html));
  assert.ok(html.includes('title="Attention mode: review">Reviewing the work</span>'));
  assert.ok(html.includes("Replaced"));
  assert.ok(!html.includes("Superseded"));

  const graph = renderToStaticMarkup(React.createElement(WorkGraphGraphView, {
    items: [
      item("draft", "2026-07-08T08:00:00Z"),
      item("final", "2026-07-08T08:10:00Z"),
      item("notes", "2026-07-08T08:20:00Z"),
    ],
    edges: [
      { kind: "supersedes", from_id: "final", to_id: "draft" },
      { kind: "derived_from", from_id: "notes", to_id: "final" },
    ],
    attention: [],
  }));
  const labels = Array.from(
    graph.matchAll(/data-testid="workgraph-graph-edge-label"[^>]*>([^<]*)<\/text>/g),
    (match) => match[1],
  ).sort();
  assert.deepEqual(labels, ["derived from", "replaces"]);
  // The raw kinds stay on the edges for selectors and styles.
  assert.ok(graph.includes('data-kind="supersedes"') && graph.includes('data-kind="derived_from"'));
});
