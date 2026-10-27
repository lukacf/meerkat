import { describe, expect, test } from "./test-support/vitest-shim";
import {
  conversationEntryText,
  conversationIdentityGroupKey,
  groupConversationTimelineEntries,
  type ConversationTimelineEntry,
} from "./conversation";

describe("conversation grouping", () => {
  test("keeps adjacent entries from the same participant grouped together", () => {
    const entries: ConversationTimelineEntry[] = [
      {
        id: "builder-1",
        kind: "message",
        variant: "plain",
        identity: {
          id: "builder",
          label: "Builder",
          role: "other",
          presentation: "participant",
          showLabel: true,
        },
        text: "I started the refactor.",
      },
      {
        id: "builder-2",
        kind: "message",
        variant: "plain",
        identity: {
          id: "builder",
          label: "Builder",
          role: "other",
          presentation: "participant",
          showLabel: true,
        },
        text: "The transcript API is normalized now.",
      },
    ];

    const groups = groupConversationTimelineEntries(entries);

    expect(groups).toHaveLength(1);
    expect(groups[0]?.entries).toHaveLength(2);
    expect(conversationIdentityGroupKey(groups[0]?.identity)).toBe("builder:participant:label");
  });

  test("splits groups when adjacent entries change presentation", () => {
    const entries: ConversationTimelineEntry[] = [
      {
        id: "assistant-1",
        kind: "message",
        variant: "plain",
        identity: {
          id: "assistant",
          label: "Assistant",
          role: "assistant",
          presentation: "assistant",
          showLabel: false,
        },
        text: "I updated the shared package.",
      },
      {
        id: "assistant-2",
        kind: "message",
        variant: "meta",
        identity: {
          id: "assistant",
          label: "System",
          role: "assistant",
          presentation: "system",
          showLabel: true,
        },
        text: "Checkpoint saved.",
      },
    ];

    const groups = groupConversationTimelineEntries(entries);

    expect(groups).toHaveLength(2);
    expect(groups[0]?.identity.presentation).toBe("assistant");
    expect(groups[1]?.identity.presentation).toBe("system");
  });

  test("keeps an assistant group anchored when an earlier same-identity entry arrives", () => {
    const user: ConversationTimelineEntry = {
      id: "user-message-1",
      kind: "message",
      variant: "plain",
      identity: {
        id: "user",
        label: "You",
        role: "user",
        presentation: "user",
      },
      text: "Ask the peer for a critique.",
    };
    const response: ConversationTimelineEntry = {
      id: "assistant-run-1",
      kind: "message",
      variant: "rich",
      identity: {
        id: "assistant",
        label: "Assistant",
        role: "assistant",
        presentation: "assistant",
      },
      blocks: [{ type: "paragraph", text: "The peer recommends more breathing room." }],
    };
    const peerTool: ConversationTimelineEntry = {
      id: "peer-tool-1",
      kind: "message",
      variant: "rich",
      identity: response.identity,
      blocks: [{
        type: "tool-call",
        toolCallId: "peer-call-1",
        name: "send_request",
        arguments: "{}",
        status: "success",
      }],
    };

    const liveGroups = groupConversationTimelineEntries([user, response]);
    const durableGroups = groupConversationTimelineEntries([user, peerTool, response]);

    expect(durableGroups[0]?.id).toBe(liveGroups[0]?.id);
    expect(durableGroups[1]?.id).toBe(liveGroups[1]?.id);
    expect(durableGroups[1]?.entries.map((entry) => entry.id)).toEqual([
      "peer-tool-1",
      "assistant-run-1",
    ]);
  });

  test("keeps a run-backed response group anchored when a late peer tool lands before a system group", () => {
    const user: ConversationTimelineEntry = {
      id: "user-message-1",
      kind: "message",
      variant: "plain",
      identity: {
        id: "user",
        label: "You",
        role: "user",
        presentation: "user",
      },
      text: "Ask the peer for a critique.",
    };
    const system: ConversationTimelineEntry = {
      id: "system-message-1",
      kind: "message",
      variant: "meta",
      identity: {
        id: "assistant",
        label: "System",
        role: "assistant",
        presentation: "system",
      },
      text: "Tool activity completed.",
    };
    const response: ConversationTimelineEntry = {
      id: "assistant-run-1",
      kind: "message",
      variant: "rich",
      identity: {
        id: "assistant",
        label: "Assistant",
        role: "assistant",
        presentation: "assistant",
      },
      runId: "run-1",
      reconciliationKey: "assistant-turn-user-message-1-response-0",
      blocks: [{ type: "paragraph", text: "The peer recommends more breathing room." }],
    };
    const peerTool: ConversationTimelineEntry = {
      id: "peer-tool-1",
      kind: "message",
      variant: "rich",
      identity: response.identity,
      interactionId: "peer-tool-interaction",
      runId: "peer-tool-run",
      reconciliationKey: "peer-tool-reconciliation",
      blocks: [{
        type: "tool-call",
        toolCallId: "peer-call-1",
        name: "send_request",
        arguments: "{}",
        status: "success",
      }],
    };

    const liveGroups = groupConversationTimelineEntries([user, system, response]);
    const durableGroups = groupConversationTimelineEntries([user, peerTool, system, response]);
    const liveResponseGroup = liveGroups.find((group) => group.entries.includes(response));
    const durableResponseGroup = durableGroups.find((group) => group.entries.includes(response));

    expect(durableResponseGroup?.id).toBe(liveResponseGroup?.id);
    expect(durableResponseGroup?.id).toContain("reconciliation-assistant-turn-user-message-1-response-0");
    expect(durableResponseGroup?.id).not.toContain("peer-tool-interaction");
    expect(durableResponseGroup?.id).not.toContain("peer-tool-reconciliation");
  });

  test("anchors one assistant group across activity artifacts and its response without conflating entry identity", () => {
    const identity = {
      id: "assistant",
      label: "Assistant",
      role: "assistant" as const,
      presentation: "assistant" as const,
    };
    const user: ConversationTimelineEntry = {
      id: "user-tool-loop",
      kind: "message",
      variant: "plain",
      identity: {
        id: "user",
        label: "You",
        role: "user",
        presentation: "user",
      },
      text: "Inspect package.json.",
    };
    const activity: ConversationTimelineEntry = {
      id: "activity-tool-loop",
      kind: "message",
      variant: "rich",
      identity,
      groupReconciliationKey: "tool-loop-response",
      blocks: [{ type: "thinking", label: "Thinking…", text: "" }],
    };
    const tool: ConversationTimelineEntry = {
      id: "tool-tool-loop",
      kind: "message",
      variant: "rich",
      identity,
      groupReconciliationKey: "tool-loop-response",
      blocks: [{
        type: "tool-call",
        toolCallId: "read-package",
        name: "cat",
        arguments: "package.json",
        status: "success",
      }],
    };
    const response: ConversationTimelineEntry = {
      id: "response-tool-loop",
      kind: "message",
      variant: "rich",
      identity,
      reconciliationKey: "tool-loop-response",
      groupReconciliationKey: "tool-loop-response",
      blocks: [{ type: "paragraph", text: "The renderer uses React." }],
    };

    const pending = groupConversationTimelineEntries([user, activity]);
    const withTool = groupConversationTimelineEntries([user, tool, activity]);
    const withResponse = groupConversationTimelineEntries([user, tool, activity, response]);

    expect(withTool[1]?.id).toBe(pending[1]?.id);
    expect(withResponse[1]?.id).toBe(pending[1]?.id);
    expect(withResponse[1]?.id).toContain("group-reconciliation-tool-loop-response");
    expect(new Set(withResponse[1]?.entries.map((entry) => entry.id)).size).toBe(3);
  });
});

describe("system task entries", () => {
  test("preserves the exact task prompt for transcript copy and accessibility surfaces", () => {
    const prompt = "Study the assigned domain.\n\nLearn every boundary and invariant.";
    const entry: ConversationTimelineEntry = {
      id: "system-task-domain-study",
      kind: "message",
      variant: "plain",
      identity: {
        id: "system-task-domain-study",
        label: "Initial domain study",
        role: "system",
        presentation: "system",
      },
      text: prompt,
      taskKind: "domain_reconnaissance",
      taskLabel: "Initial domain study",
      taskId: "domain-study-runtime",
      taskStatus: "running",
      runId: "run-domain-study-runtime",
    };

    expect(conversationEntryText(entry)).toBe(prompt);
    expect(conversationIdentityGroupKey(entry.identity)).toBe("system-task-domain-study:system:label");
  });
});

describe("flow run entries", () => {
  test("preserves an honest stopped state in the shared conversation model", () => {
    const entry: ConversationTimelineEntry = {
      id: "flow-run:stopped",
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

    expect(entry.status).toBe("stopped");
    expect(conversationEntryText(entry)).toContain("Reviewer: Stopped by the operator");
  });
});

describe("workgraph entry text", () => {
  test("projects the goal, indented item tree, and attention rows for copy surfaces", () => {
    const entry: ConversationTimelineEntry = {
      kind: "workgraph",
      id: "workgraph:goal-1",
      identity: { id: "planner", label: "Planner", role: "assistant" },
      rootId: "goal-1",
      title: "Release 0.7.30",
      objective: "Ship WorkGraph end to end",
      status: "active",
      progress: { completed: 1, total: 2 },
      items: [
        {
          itemId: "goal-1",
          title: "Release 0.7.30",
          status: "in_progress",
          revision: 4,
          depth: 0,
        },
        {
          itemId: "child-1",
          title: "Console card",
          status: "completed",
          revision: 2,
          depth: 1,
          parentId: "goal-1",
        },
      ],
      attention: [
        {
          bindingId: "attention-1",
          mode: "pursue",
          statusLabel: "active",
          targetLabel: "sess-42",
          revision: 7,
        },
      ],
    };

    expect(conversationEntryText(entry)).toBe([
      "Release 0.7.30 (1/2)",
      "Ship WorkGraph end to end",
      "Release 0.7.30 — in progress",
      "  Console card — completed",
      "pursue: active → sess-42",
    ].join("\n"));
  });
});

describe("group id contract (PRs #281-#290 audit regressions)", () => {
  const user = (id: string): ConversationTimelineEntry => ({
    id,
    kind: "message",
    variant: "plain",
    identity: { id: "user", label: "You", role: "user", presentation: "user", showLabel: false },
    text: "hello",
  });
  const assistant = (
    id: string,
    extra: Partial<ConversationTimelineEntry> = {},
  ): ConversationTimelineEntry => ({
    id,
    kind: "message",
    variant: "plain",
    identity: { id: "assistant", label: "Agent", role: "assistant", presentation: "assistant", showLabel: false },
    text: `text-${id}`,
    ...extra,
  } as ConversationTimelineEntry);
  const systemMeta = (id: string): ConversationTimelineEntry => ({
    id,
    kind: "message",
    variant: "meta",
    identity: { id: "system", label: "System", role: "other", presentation: "participant", showLabel: true },
    text: "meta",
  });

  test("two same-identity groups sharing an interaction id never collide (React keys)", () => {
    const groups = groupConversationTimelineEntries([
      user("u-1"),
      assistant("a-tool", { interactionId: "interaction-x" }),
      systemMeta("meta-1"),
      assistant("a-response", { interactionId: "interaction-x" }),
    ]);
    const ids = groups.map((group) => group.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  test("host groupReconciliationKey beats an earlier entry's provisional interactionId", () => {
    const groups = groupConversationTimelineEntries([
      user("u-1"),
      assistant("a-thinking", { interactionId: "live-provisional-1" }),
      assistant("a-response", { groupReconciliationKey: "turn-7-response" }),
    ]);
    expect(groups[1]?.id).toContain("group-reconciliation-turn-7-response");
  });

  test("keyless group ids survive a late-inserted earlier sibling group", () => {
    const before = groupConversationTimelineEntries([
      user("u-1"),
      assistant("a-response"),
    ]);
    const after = groupConversationTimelineEntries([
      user("u-1"),
      assistant("a-late-peer", {
        variant: "rich",
        blocks: [{ type: "tool-call", name: "send_message", state: "complete" }],
      } as Partial<ConversationTimelineEntry>),
      systemMeta("meta-1"),
      assistant("a-response"),
    ]);
    const responseBefore = before.at(-1);
    const responseAfter = after.at(-1);
    expect(responseAfter?.id).toBe(responseBefore?.id);
  });

  test("user-less conversations key groups without positional drift", () => {
    const before = groupConversationTimelineEntries([
      assistant("a-1"),
      systemMeta("m-1"),
      assistant("a-2"),
    ]);
    const after = groupConversationTimelineEntries([
      assistant("a-0"),
      systemMeta("m-0"),
      assistant("a-1"),
      systemMeta("m-1"),
      assistant("a-2"),
    ]);
    expect(after.at(-1)?.id).toBe(before.at(-1)?.id);
  });

  test("an anchored streaming group keeps its id when the terminal entry joins", () => {
    const streaming = groupConversationTimelineEntries([
      user("u-1"),
      assistant("live-text-1", { interactionId: "interaction-x" }),
    ]);
    const finalized = groupConversationTimelineEntries([
      user("u-1"),
      assistant("live-text-1", { interactionId: "interaction-x" }),
      assistant("terminal-1", { interactionId: "interaction-x" }),
    ]);
    expect(finalized[1]?.id).toBe(streaming[1]?.id);
  });
});

describe("assistant response ownership boundaries", () => {
  const response = (id: string, owner: { interactionId?: string; runId?: string; groupReconciliationKey?: string } = {}): ConversationTimelineEntry => ({
    id, kind: "message", variant: "plain",
    identity: { id: "router:main", label: "Router", role: "assistant" },
    text: "The checks passed.", ...owner,
  });

  test("separates identical substantive replies from distinct typed interactions", () => {
    const first = response("first", { interactionId: "review-1" });
    const next = response("next", { interactionId: "review-2" });
    const groups = groupConversationTimelineEntries([first, next]);
    expect(groups.map(group => group.entries.map(entry => entry.id))).toEqual([["first"], ["next"]]);
    expect(groups.map(group => group.copyText)).toEqual(["The checks passed.", "The checks passed."]);
    expect(new Set(groups.map(group => group.id)).size).toBe(2);
  });

  test("separates typed runs even when they share one interaction", () => {
    const first = response("first", { interactionId: "review", runId: "run-1" });
    const next = response("next", { interactionId: "review", runId: "run-2" });
    const groups = groupConversationTimelineEntries([first, next]);
    expect(groups.map(group => group.entries.map(entry => entry.id))).toEqual([["first"], ["next"]]);
    expect(new Set(groups.map(group => group.id)).size).toBe(2);
    const later = response("next-final", { interactionId: "review", runId: "run-2" });
    const peerTool: ConversationTimelineEntry = {
      ...response("late-peer", { interactionId: "peer", runId: "peer-run" }),
      kind: "message", variant: "rich", blocks: [{ type: "tool-call", name: "send_request", status: "success" }],
    };
    expect(groupConversationTimelineEntries([peerTool, first, next, later]).map(group => group.id))
      .toEqual(groups.map(group => group.id));
    const durable = groupConversationTimelineEntries([
      { ...first, id: "durable-first" },
      { ...next, id: "durable-next" },
    ]);
    expect(durable.map(group => group.id)).toEqual(groups.map(group => group.id));
  });

  test("keeps same-run chunks and unscoped compatibility entries without bridging distinct owners", () => {
    const groups = groupConversationTimelineEntries([
      response("first", { interactionId: "review-1", runId: "run-1" }),
      response("same-run", { interactionId: "review-1", runId: "run-1" }),
      response("unscoped"),
      response("next", { interactionId: "review-2", runId: "run-2" }),
    ]);
    expect(groups.map(group => group.entries.map(entry => entry.id))).toEqual([["first", "same-run", "unscoped"], ["next"]]);
    expect(groupConversationTimelineEntries([response("legacy-a"), response("legacy-b")])).toHaveLength(1);
  });

  test("late run metadata cannot change a stable interaction group anchor", () => {
    const initial = response("live", { interactionId: "review" });
    const durable = response("durable", { interactionId: "review", runId: "run-1" });
    expect(groupConversationTimelineEntries([durable])[0]?.id)
      .toBe(groupConversationTimelineEntries([initial])[0]?.id);
  });

  test("distinct interactions sharing a run retain stable independent group identities", () => {
    const entries = [
      response("first", { interactionId: "review-1", runId: "run-1" }),
      response("next", { interactionId: "review-2", runId: "run-1" }),
    ];
    const live = groupConversationTimelineEntries(entries);
    const durable = groupConversationTimelineEntries(entries.map(entry => ({ ...entry, id: `durable-${entry.id}` })));
    expect(new Set(live.map(group => group.id)).size).toBe(2);
    expect(durable.map(group => group.id)).toEqual(live.map(group => group.id));
  });

  test("late tool and meta owners cannot split or re-key their substantive response", () => {
    const first = response("first", { interactionId: "review-1", runId: "run-1", groupReconciliationKey: "response-1" });
    const final = response("final", { interactionId: "review-1", runId: "run-1", groupReconciliationKey: "response-1" });
    const next = response("next", { interactionId: "review-2", runId: "run-2", groupReconciliationKey: "response-2" });
    const tool: ConversationTimelineEntry = {
      ...response("peer-tool", { interactionId: "peer", runId: "peer-run", groupReconciliationKey: "peer-group" }),
      kind: "message", variant: "rich", blocks: [{ type: "tool-call", name: "send_message", status: "success" }],
    };
    const meta: ConversationTimelineEntry = { ...response("meta", { interactionId: "meta-owner", runId: "meta-run" }), kind: "message", variant: "meta" };
    const before = groupConversationTimelineEntries([first, final, next]);
    const after = groupConversationTimelineEntries([tool, first, meta, final, next]);
    expect(after.map(group => group.entries.map(entry => entry.id))).toEqual([["peer-tool", "first", "meta", "final"], ["next"]]);
    expect(after.map(group => group.id)).toEqual(before.map(group => group.id));
  });

  test("explicit shared host group keys keep provisional and durable response ownership together", () => {
    const provisional = response("activity", { interactionId: "provisional", runId: "provisional-run", groupReconciliationKey: "host-response" });
    const durable = response("response", { interactionId: "canonical", runId: "canonical-run", groupReconciliationKey: "host-response" });
    const groups = groupConversationTimelineEntries([provisional, durable]);
    expect(groups).toHaveLength(1);
    expect(groups[0]?.id).toBe(groupConversationTimelineEntries([provisional])[0]?.id);
    expect(groups[0]?.id).toContain("group-reconciliation-host-response");
  });

  test("a host group anchor covers intervening unkeyed chunks during durable owner reconciliation", () => {
    const provisional = response("activity", { interactionId: "provisional", runId: "provisional-run", groupReconciliationKey: "host-response" });
    const chunk = response("chunk", { interactionId: "provisional", runId: "provisional-run" });
    const durable = response("response", { interactionId: "canonical", runId: "canonical-run", groupReconciliationKey: "host-response" });
    const before = groupConversationTimelineEntries([provisional, chunk]);
    const after = groupConversationTimelineEntries([provisional, chunk, durable]);
    expect(after.map(group => group.entries.map(entry => entry.id))).toEqual([["activity", "chunk", "response"]]);
    expect(after.map(group => group.id)).toEqual(before.map(group => group.id));
  });
});
