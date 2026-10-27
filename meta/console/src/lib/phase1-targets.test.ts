import test from "node:test";
import assert from "node:assert/strict";
import {
  normalizeToolCallAccumulatorState,
  type ToolCallAccumulatorState,
} from "@console-core";
import { parseSseFrames } from "./network";
import {
  buildDockTarget,
  buildPanelConversationKey,
  buildRoutingSectionView,
  buildSidebarViewState,
  mapFramesToTimelineEntries,
} from "./adapters";
import { normalizeAgents } from "./agents";
import type { ConsoleAgent, ConsoleExperience } from "../types";

test("CHOKE-002 target: one timeline stream fans out to multiple panel consumers without divergent frame identity", () => {
  const rawSse = [
    "id: evt-1",
    "event: console_frame",
    'data: {"type":"console_frame","frame":{"id":"evt-1","cursor":"console:1","dedupe_key":"evt-1","runtime_key":"default","identity":"identity:luka","kind":"user_input","status":"accepted","timestamp_ms":1,"payload":{}}}',
    "",
  ].join("\n");

  const panelAFrames = parseSseFrames(rawSse);
  const panelBFrames = parseSseFrames(rawSse);

  assert.equal(panelAFrames[0]?.id, "evt-1");
  assert.equal(panelBFrames[0]?.id, "evt-1");
  assert.deepEqual(panelAFrames[0], panelBFrames[0]);
});

test("CHOKE-004 target: sidebar adapter chooses identity addressing once for composer send flow", () => {
  const target = buildDockTarget({
    identity: "identity:luka",
    member_id: "member-luka",
    agent_id: "member-luka",
    label: "Luka",
    kind: "identity",
    addressable: true,
  });

  assert.equal(target.addressingMode, "identity");
  assert.equal(target.identity, "identity:luka");
});

test("sidebar adapter keeps durable chat identity when member id is runtime-scoped", () => {
  const [agent] = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            {
              identity: "review:singleton",
              member_id: "rt:review:singleton:0",
              agent_id: "rt:review:singleton:0",
              label: "Review Worker",
              kind: "mob_agent",
              labels: { agent_identity: "review:singleton" },
            },
          ],
        },
      },
    } as ConsoleExperience,
    [],
  );

  assert.ok(agent);
  const target = buildDockTarget(agent);

  assert.equal(target.identity, "review:singleton");
  assert.equal(target.memberId, "rt:review:singleton:0");
});

test("CHOKE-003 target: refreshed experience metadata drives host refresh strategy instead of stale per-panel fetch assumptions", () => {
  const before = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: {
          agents: [
            { member_id: "legacy-router", agent_id: "legacy-router", label: "Legacy Router", kind: "module_agent" },
          ],
        },
      },
    } as ConsoleExperience,
    [],
  );
  const after = normalizeAgents(
    {
      agent_sidebar: {
        live_snapshot: { agents: [] },
      },
      identity_status: {
        refresh: { mode: "stream", interval_ms: 1000 },
        rows: [
          {
            identity: "identity:luka",
            display_name: "Luka",
            role: "lead",
            state: "running",
            addressability: "addressable",
            labels: {},
          },
        ],
      },
    } as ConsoleExperience,
    [],
  );

  assert.equal(before[0]?.member_id, "legacy-router");
  assert.equal(after[0]?.identity, "identity:luka");
});

test("CHOKE-006 / E2E-008 target: out-of-order tool results pair into stable transcript blocks", () => {
  const accumulator = normalizeToolCallAccumulatorState({
    timeoutMs: 60000,
    toolCalls: {
      "tool-1": {
        type: "tool-call",
        toolCallId: "tool-1",
        name: "search",
        arguments: "{\"q\":\"luka\"}",
        result: "{\"hits\":1}",
        status: "success",
      },
    },
    pendingResults: {},
  }) as ToolCallAccumulatorState | null;

  assert.ok(accumulator);
  assert.equal(Object.keys(accumulator?.toolCalls || {}).length, 1);
  assert.equal(accumulator?.toolCalls["tool-1"]?.status, "success");
  assert.equal(accumulator?.toolCalls["tool-1"]?.toolCallId, "tool-1");
});

test("CHOKE-009 / E2E-010 target: routing panel owns generic route and delivery projection", () => {
  const view = buildRoutingSectionView({
    routesResponse: {
      routes: [
        {
          route_key: "route-1",
          channel: "email",
          recipient: "user@example.com",
          sink: "delivery/email",
          target_module: "delivery",
          source: "runtime",
        },
      ],
    },
    historyResponse: {
      deliveries: [
        {
          delivery_id: "delivery-1",
          route_id: "route-1",
          recipient: "user@example.com",
          sink: "delivery/email",
          target_module: "delivery",
          status: "delivered",
          first_attempt_ms: 1000,
          final_attempt_ms: 1000,
          attempts: [{ attempt: 1, status: "delivered", backoff_ms: 0 }],
        },
      ],
    },
  });

  assert.equal(view.routes.length, 1);
  assert.equal(view.deliveries.length, 1);
  assert.equal(view.deliveries[0]?.attempts.length, 1);
});

test("CHOKE-011 / E2E-011 target: watch and degraded state converge into one shared sidebar item model", () => {
  const view = buildSidebarViewState({
    selectedMemberId: "member-luka",
    agents: [
      {
        member_id: "member-luka",
        agent_id: "member-luka",
        identity: "identity:luka",
        label: "Luka",
        kind: "identity",
        watched: true,
        alertLevel: "critical",
        degraded: true,
        degradedReason: "lease_expired",
      },
    ],
  });

  const item = view.blocks[1]?.kind === "list"
    ? view.blocks[1].sections[0]?.items[0]
    : undefined;
  assert.equal(item?.watched, true);
  assert.equal(item?.alertLevel, "critical");
  assert.equal(item?.degraded, true);
  assert.equal(item?.degradedReason, "lease_expired");
});

test("CHOKE-016 target: dock targets always resolve to identity addressing", () => {
  const beforeRefresh = buildDockTarget({
    member_id: "member-luka",
    agent_id: "member-luka",
    label: "Luka",
    kind: "module_agent",
  });
  const afterRefresh = buildDockTarget({
    identity: "identity:luka",
    member_id: "member-luka",
    agent_id: "member-luka",
    label: "Luka",
    kind: "identity",
  });

  assert.equal(beforeRefresh.addressingMode, "identity");
  assert.equal(beforeRefresh.identity, "member-luka");
  assert.equal(afterRefresh.addressingMode, "identity");
  assert.equal(afterRefresh.identity, "identity:luka");
});

test("E2E-001 target: legacy fallback still yields a usable sidebar", () => {
  const agents = normalizeAgents(
    {
      agent_sidebar: {
        title: "Agents",
        live_snapshot: {
          agents: [
            {
              member_id: "router",
              agent_id: "router",
              label: "Router",
              kind: "module_agent",
            },
          ],
        },
      },
    } as ConsoleExperience,
    [],
  );

  assert.equal(agents.length, 1);
  assert.equal(agents[0]?.member_id, "router");
});

test("E2E-014 target: tool timeouts surface as timed-out tool blocks instead of disappearing", () => {
  const accumulator = normalizeToolCallAccumulatorState({
    timeoutMs: 60000,
    toolCalls: {
      "tool-timeout": {
        type: "tool-call",
        toolCallId: "tool-timeout",
        name: "slow_search",
        arguments: "{\"q\":\"timeout\"}",
        status: "error",
        result: "{\"error\":\"timed_out\"}",
      },
    },
    pendingResults: {},
  }) as ToolCallAccumulatorState | null;

  assert.ok(accumulator);
  assert.equal(accumulator?.toolCalls["tool-timeout"]?.status, "error");
  assert.equal(accumulator?.toolCalls["tool-timeout"]?.toolCallId, "tool-timeout");
});

test("E2E-016 target: overflow recovery keeps the host on replay-based recovery instead of an unbounded local queue", () => {
  const frames = buildSidebarViewState({
    selectedMemberId: "member-luka",
    agents: [
      {
        member_id: "member-luka",
        agent_id: "member-luka",
        label: "Luka",
        kind: "identity",
        watched: true,
      },
    ],
  });

  assert.equal(frames.blocks[1]?.kind, "list");
});

test("E2E-017 target: mixed migration sessions still produce identity-addressed targets", () => {
  const identityTarget = buildDockTarget({
    identity: "identity:luka",
    member_id: "member-luka",
    agent_id: "member-luka",
    label: "Luka",
    kind: "identity",
  });
  const legacyTarget = buildDockTarget({
    member_id: "legacy-router",
    agent_id: "legacy-router",
    label: "Legacy Router",
    kind: "module_agent",
  });

  assert.equal(identityTarget.addressingMode, "identity");
  assert.equal(legacyTarget.addressingMode, "identity");
  assert.equal(legacyTarget.identity, "legacy-router");
});

test("terminal identity events surface transcript payloads instead of disappearing", () => {
  const agent: ConsoleAgent = {
    member_id: "member-luka",
    agent_id: "member-luka",
    label: "Luka",
    kind: "identity",
  };

  const successEntries = mapFramesToTimelineEntries(agent, [
    { id: "evt-1", event: "interaction_complete", data: { text: "done" } },
  ]);
  assert.equal(successEntries.length, 1);
  assert.equal(successEntries[0]?.identity.role, "assistant");

  const failureEntries = mapFramesToTimelineEntries(agent, [
    { id: "evt-2", event: "interaction_failed", data: { reason: "lifecycle_mutation" } },
  ]);
  assert.equal(failureEntries.length, 1);
  assert.equal(failureEntries[0]?.variant, "meta");
});

test("Panel-state target: same-target split panels and retargeted panels keep distinct local composer/transcript state keys", () => {
  const identityTarget = {
    id: "member-luka",
    kind: "agent-chat" as const,
    addressingMode: "identity" as const,
    memberId: "member-luka",
    identity: "identity:luka",
    title: "Luka",
  };
  const legacyTarget = {
    id: "legacy-router",
    kind: "agent-chat" as const,
    addressingMode: "identity" as const,
    memberId: "legacy-router",
    identity: "legacy-router",
    title: "Legacy Router",
  };

  const splitPanelA = buildPanelConversationKey("panel-a", identityTarget);
  const splitPanelB = buildPanelConversationKey("panel-b", identityTarget);
  const retargetedPanel = buildPanelConversationKey("panel-a", legacyTarget);

  assert.notEqual(splitPanelA, splitPanelB);
  assert.notEqual(splitPanelA, retargetedPanel);
  assert.equal(splitPanelA, "panel:panel-a:agent-chat:identity:luka");
  assert.equal(splitPanelB, "panel:panel-b:agent-chat:identity:luka");
  assert.equal(retargetedPanel, "panel:panel-a:agent-chat:legacy-router");
});
