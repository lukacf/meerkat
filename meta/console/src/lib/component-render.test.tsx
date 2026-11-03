import assert from "node:assert/strict";
import test from "node:test";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";

import {
  groupConversationTimelineEntries,
  normalizeConsoleNavigationModel,
  type ConversationFlowRunEntry,
  type ConversationTimelineEntry,
} from "@console-core";
import {
  ConsoleComposer,
  ConsoleWorkbench,
  ConversationTranscript,
  WorkGraphCard,
} from "@console-components";

function HorizontalNavigation() {
  const model = normalizeConsoleNavigationModel({
    orientation: "horizontal",
    activeNodeId: "thread:alpha",
    nodes: [
      {
        type: "group",
        id: "projects",
        label: "Projects",
        expanded: true,
        children: [
          { type: "item", id: "project:alpha", label: "Project Alpha" },
          { type: "item", id: "thread:alpha", label: "Planning Thread", selected: true },
        ],
      },
    ],
    order: { orderedNodeIds: [] },
  });

  return (
    <nav aria-label="Host navigation" data-orientation={model.orientation}>
      {model.nodes.flatMap((node) => node.type === "group" ? node.children : [node]).map((node) => (
        <button aria-current={node.selected ? "page" : undefined} key={node.id} type="button">
          {node.label}
        </button>
      ))}
    </nav>
  );
}

test("alternate shell renders non-sidebar navigation with Meta transcript and composer components", () => {
  const entries: ConversationTimelineEntry[] = [
    {
      id: "user-1",
      kind: "message",
      variant: "plain",
      identity: { id: "user", label: "You", role: "user", presentation: "user" },
      text: "Use a horizontal host navigator.",
    },
    {
      id: "assistant-1",
      kind: "message",
      variant: "plain",
      identity: { id: "assistant", label: "Assistant", role: "assistant" },
      text: "Meta transcript and composer still render.",
    },
  ];
  const html = renderToStaticMarkup(
    <ConsoleWorkbench
      launcher={<HorizontalNavigation />}
      main={(
        <ConversationTranscript
          viewState={{
            conversationId: "fixture",
            entries,
            groups: groupConversationTimelineEntries(entries),
            turnDiff: null,
            emptyState: null,
          }}
        />
      )}
      mainFooter={(
        <ConsoleComposer
          viewState={{
            value: "next prompt",
            placeholder: "Send to the selected Meta target",
            mainRowItems: [],
            footerLeftItems: [],
            footerRightItems: [],
          }}
          onChange={() => undefined}
          onSubmit={() => undefined}
        />
      )}
    />,
  );

  assert.match(html, /data-orientation="horizontal"/);
  assert.match(html, /Project Alpha/);
  assert.match(html, /Planning Thread/);
  assert.match(html, /Meta transcript and composer still render/);
  assert.match(html, /Send to the selected Meta target/);
});

test("council entries render as cards through the SHARED transcript, not only in ChatPane", () => {
  // Regression: ConversationMessageView is the shared render path (group ->
  // transcript -> pane). Without a council branch there, the card appeared in
  // the app that wires it explicitly and rendered NOTHING in every other
  // consumer - a council entry carries no text and no blocks, so the generic
  // message fallback is empty.
  const councilEntry: ConversationTimelineEntry = {
    kind: "council",
    id: "council:c-1",
    identity: { id: "planner", label: "Planner", role: "assistant" },
    councilId: "c-1",
    topic: "Ship or hold 0.8.28?",
    status: "failed",
    exitReason: "exchange_failed",
    exitDetail: "round 2 · provider timeout",
    roundsCompleted: 1,
    participants: [
      {
        order: 0,
        role: "critic",
        sourceMobId: "m1",
        sourceIdentity: "alice",
        targetIdentity: "alice#fork",
        seated: true,
      },
    ],
    exchanges: [],
  };

  const html = renderToStaticMarkup(
    <ConversationTranscript
      viewState={{
        conversationId: "fixture",
        entries: [councilEntry],
        groups: groupConversationTimelineEntries([councilEntry]),
        turnDiff: null,
        emptyState: null,
      }}
    />,
  );

  assert.match(html, /data-council-card/);
  assert.match(html, /data-council-id="c-1"/);
  assert.match(html, /data-status="failed"/);
  assert.match(html, /Ship or hold 0\.8\.28\?/);
  // The typed reason and its detail must survive the shared path too.
  assert.match(html, /provider timeout/);
  // Observational by construction: no operator affordances anywhere.
  assert.doesNotMatch(html, /council-action:/);
});

test("workgraph entries render as cards through the shared transcript, with action buttons gated on callbacks", () => {
  const workGraphEntry: ConversationTimelineEntry = {
    kind: "workgraph",
    id: "workgraph:goal-1",
    identity: { id: "planner", label: "Planner", role: "assistant" },
    rootId: "goal-1",
    title: "Release 0.7.30",
    objective: "Ship WorkGraph end to end",
    status: "active",
    progress: { completed: 1, total: 2 },
    items: [
      { itemId: "goal-1", title: "Release 0.7.30", status: "in_progress", revision: 4, depth: 0 },
      { itemId: "child-1", title: "Console card", status: "open", revision: 1, depth: 1, parentId: "goal-1" },
    ],
    attention: [
      { bindingId: "attention-1", mode: "pursue", statusLabel: "active", targetLabel: "sess-42", revision: 7 },
    ],
  };

  const observedHtml = renderToStaticMarkup(
    <ConversationTranscript
      viewState={{
        conversationId: "fixture",
        entries: [workGraphEntry],
        groups: groupConversationTimelineEntries([workGraphEntry]),
        turnDiff: null,
        emptyState: null,
      }}
    />,
  );

  assert.match(observedHtml, /data-work-graph-card/);
  assert.match(observedHtml, /data-root-id="goal-1"/);
  assert.match(observedHtml, /data-status="active"/);
  assert.match(observedHtml, /Release 0\.7\.30/);
  assert.match(observedHtml, /1\/2/);
  assert.match(observedHtml, /pursue/);
  // Observational transcript: no callbacks, no operator buttons.
  assert.doesNotMatch(observedHtml, /workgraph-action:/);
  assert.doesNotMatch(observedHtml, /workgraph-attention:/);

  const managedHtml = renderToStaticMarkup(
    <WorkGraphCard
      entry={workGraphEntry}
      actions={{
        onClaim: () => undefined,
        onClose: () => undefined,
        onAttentionPause: () => undefined,
      }}
    />,
  );

  assert.match(managedHtml, /data-testid="workgraph-action:child-1:claim"/);
  assert.match(managedHtml, /data-testid="workgraph-action:goal-1:close"/);
  assert.match(managedHtml, /data-testid="workgraph-attention:attention-1:pause"/);
  // Callbacks not provided render no affordance.
  assert.doesNotMatch(managedHtml, /workgraph-attention:attention-1:resume/);
  assert.doesNotMatch(managedHtml, /workgraph-attention:attention-1:confirm/);
  // No failure flag → no failed indicator.
  assert.doesNotMatch(managedHtml, /workgraph-card:goal-1:last-action-failed/);
});

test("flow-run cards render stopped state, semantic static rows, and uniquely named message actions", () => {
  const flowRunEntry: ConversationFlowRunEntry = {
    id: "flow-run:release-crew",
    kind: "flow_run",
    identity: { id: "coordinator", label: "Coordinator", role: "assistant" },
    helperId: "helper-1",
    flowName: "Release crew",
    status: "stopped",
    rows: [
      {
        memberKey: "reviewer",
        label: "Reviewer",
        caption: "Stopped by the operator",
        status: "stopped",
      },
    ],
  };
  const html = renderToStaticMarkup(
    <ConversationTranscript
      viewState={{
        conversationId: "fixture",
        entries: [flowRunEntry],
        groups: groupConversationTimelineEntries([flowRunEntry]),
        turnDiff: null,
        emptyState: null,
      }}
      onFlowRunMessageMember={() => undefined}
    />,
  );

  assert.match(html, /data-flow-run-card=""/);
  assert.match(html, /data-status="stopped"/);
  assert.match(html, /cc-flow-run__badge is-stopped">Stopped/);
  assert.match(html, /<div class="cc-flow-run__member-row">/);
  assert.doesNotMatch(html, /<button[^>]+class="cc-flow-run__member-row"[^>]+disabled/);
  assert.match(html, /aria-label="Message Reviewer"/);
  assert.match(html, /cc-flow-run__member-status">Stopped/);
});

test("workgraph card gates reassign to coordinate-mode bindings and surfaces the last-action-failed flag", () => {
  const entry = (mode: string, lastActionFailed?: boolean): ConversationTimelineEntry => ({
    kind: "workgraph",
    id: "workgraph:goal-1",
    identity: { id: "planner", label: "Planner", role: "assistant" },
    rootId: "goal-1",
    title: "Release 0.7.30",
    status: "active",
    progress: { completed: 0, total: 1 },
    items: [
      { itemId: "goal-1", title: "Release 0.7.30", status: "in_progress", revision: 4, depth: 0 },
    ],
    attention: [
      { bindingId: "attention-1", mode, statusLabel: "active", revision: 7, itemId: "goal-1" },
    ],
    ...(lastActionFailed ? { lastActionFailed } : {}),
  });
  const actions = { onAttentionReassign: () => undefined };

  // Reassign authority is machine-derived from the binding mode upstream:
  // only coordinate-mode bindings render the affordance.
  const pursueHtml = renderToStaticMarkup(<WorkGraphCard entry={entry("pursue")} actions={actions} />);
  assert.doesNotMatch(pursueHtml, /workgraph-attention:attention-1:reassign/);
  const coordinateHtml = renderToStaticMarkup(<WorkGraphCard entry={entry("coordinate")} actions={actions} />);
  assert.match(coordinateHtml, /data-testid="workgraph-attention:attention-1:reassign"/);

  const failedHtml = renderToStaticMarkup(<WorkGraphCard entry={entry("pursue", true)} />);
  assert.match(failedHtml, /data-testid="workgraph-card:goal-1:last-action-failed"/);
});

test("workgraph card renders a '+N more items' overflow row when the adapter capped the item rows", () => {
  const entry: ConversationTimelineEntry = {
    kind: "workgraph",
    id: "workgraph:goal-1",
    identity: { id: "planner", label: "Planner", role: "assistant" },
    rootId: "goal-1",
    title: "Release 0.7.30",
    status: "active",
    // The adapter counts overflow items toward progress: 40 total, 30 shown.
    progress: { completed: 12, total: 40 },
    items: [
      { itemId: "goal-1", title: "Release 0.7.30", status: "in_progress", revision: 4, depth: 0 },
    ],
    itemOverflowCount: 10,
    attention: [],
  };

  const html = renderToStaticMarkup(<WorkGraphCard entry={entry} />);
  assert.match(html, /data-testid="workgraph-card:goal-1:overflow"/);
  assert.match(html, /\+10 more items/);
  assert.match(html, /12\/40/, "the progress counter reflects the full totals, not the visible rows");

  const uncapped = renderToStaticMarkup(
    <WorkGraphCard entry={{ ...entry, itemOverflowCount: undefined }} />,
  );
  assert.doesNotMatch(uncapped, /workgraph-card:goal-1:overflow/);
});
