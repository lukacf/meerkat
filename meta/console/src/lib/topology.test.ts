import assert from "node:assert/strict";
import test from "node:test";

import {
  applyConsoleDockAction,
  collectConsoleDockPanelIds,
  createConsoleDockState,
} from "@console-core";
import type { ConsoleDockAction, ConsoleDockState, ConsoleDockTarget } from "@console-core";

import {
  consoleTopologyEndpointKey,
  createConsoleTopologyMutationRequest,
  executeConsoleTopologyMutation,
  mergeTopologyOperationReceipt,
  normalizeConsoleTopologyQuery,
  normalizeTopologyOperationReceipt,
  parseConsoleTopologyEndpointKey,
  pendingTopologyReceipt,
  resolveAmbiguousConsoleTopologyMutation,
  topologyApplyParams,
  topologyAgentChatActions,
  topologyNodeAgent,
  topologyPlanParams,
  topologyTeamKey,
} from "./topology";

const AUTHORITY = "incident-command-center";
const endpointId = (identity: string, authority = AUTHORITY) =>
  consoleTopologyEndpointKey({ authority, identity });

const editableCapabilities = {
  mode: "editable" as const,
  can_query: true,
  can_plan: true,
  can_apply: true,
  can_bulk: false,
  max_batch_size: 1,
};

function query(overrides: Record<string, unknown> = {}) {
  return {
    authority: "incident-command-center",
    revision: 7,
    policy: {
      mode: "editable",
      allow_bulk: false,
      max_batch_size: 1,
    },
    nodes: [
      {
        endpoint: { identity: "incident-commander" },
        role: "commander",
        labels: { team: "coordination" },
        affordances: {
          can_connect: true,
          can_disconnect: true,
          can_reconnect: true,
          can_bulk: false,
          can_cross_authority: false,
        },
      },
      {
        endpoint: { identity: "payments-sre" },
        role: "sre",
        labels: { team: "responders" },
        affordances: {
          can_connect: true,
          can_disconnect: true,
          can_reconnect: true,
          can_bulk: false,
          can_cross_authority: false,
        },
      },
      {
        endpoint: { identity: "api-investigator" },
        role: "investigator",
        affordances: {
          can_connect: true,
          can_disconnect: false,
          can_reconnect: false,
          can_bulk: false,
          can_cross_authority: false,
        },
      },
    ],
    edges: [
      {
        edge: {
          a: { identity: "incident-commander" },
          b: { identity: "payments-sre" },
        },
        actual: true,
        declared: true,
        operator_added: false,
        suppressed: false,
        desired: true,
      },
    ],
    ...overrides,
  };
}

test("topology nodes name a nameless agent the way every surface does", () => {
  const base = query();
  const nodes = [
    ...(base.nodes as Array<Record<string, unknown>>),
    { endpoint: { identity: "router:main" }, role: "router", labels: {}, affordances: { can_connect: false, can_disconnect: false, can_reconnect: false, can_bulk: false, can_cross_authority: false } },
  ];
  const normalized = normalizeConsoleTopologyQuery(query({ nodes }), {
    capabilities: editableCapabilities,
    agents: [{ agent_id: "incident-commander", member_id: "member-commander", label: "Incident Commander", kind: "agent" }],
  });
  assert.ok(normalized);
  const label = (identity: string) => normalized.nodes.find((node) => node.ref?.identity === identity)?.label;
  assert.equal(label("router:main"), "Router main");
  assert.equal(label("incident-commander"), "Incident Commander");
});

test("topology query maps authoritative edges and intersects both endpoint grants", () => {
  const normalized = normalizeConsoleTopologyQuery(query(), {
    capabilities: editableCapabilities,
    connectionSourceId: endpointId("incident-commander"),
    agents: [
      {
        agent_id: "incident-commander",
        member_id: "member-commander",
        label: "Incident Commander",
        kind: "agent",
      },
    ],
  });
  assert.ok(normalized);
  assert.equal(normalized.management.policy.mode, "editable");
  assert.equal(normalized.management.policy.capabilities.bulk, undefined);
  assert.deepEqual(
    normalized.nodes.find((node) => node.identity === endpointId("incident-commander"))?.wired_to,
    [endpointId("payments-sre")],
  );
  assert.deepEqual(
    normalized.nodes.find((node) => node.identity === endpointId("incident-commander"))?.ref,
    {
      id: endpointId("incident-commander"),
      authority: AUTHORITY,
      identity: "incident-commander",
    },
  );
  assert.equal(
    normalized.management.affordances.find((entry) =>
      entry.edge.from === endpointId("api-investigator")
      && entry.edge.to === endpointId("incident-commander")
    )?.actions.connect?.state,
    "allowed",
  );
  assert.equal(
    normalized.management.affordances.find((entry) =>
      entry.edge.from === endpointId("api-investigator")
      && entry.edge.to === endpointId("incident-commander")
    )?.actions.disconnect?.state,
    "denied",
  );
});

test("topology query fails closed on missing endpoint affordances and global read-only", () => {
  const withoutAffordances = query({
    nodes: [
      { endpoint: { identity: "incident-commander" }, role: "commander" },
      { endpoint: { identity: "payments-sre" }, role: "sre" },
    ],
  });
  const normalized = normalizeConsoleTopologyQuery(withoutAffordances, {
    capabilities: editableCapabilities,
    connectionSourceId: endpointId("incident-commander"),
  });
  assert.ok(normalized);
  assert.equal(normalized.management.affordances[0]?.actions.connect?.state, "denied");

  const readOnly = normalizeConsoleTopologyQuery(query(), {
    capabilities: { ...editableCapabilities, can_apply: false },
    connectionSourceId: endpointId("incident-commander"),
  });
  assert.equal(readOnly?.management.policy.mode, "read_only");
  assert.ok(readOnly?.management.affordances.every((entry) =>
    Object.values(entry.actions).every((capability) => capability?.state === "denied")
  ));

  const cannotPlan = normalizeConsoleTopologyQuery(query(), {
    capabilities: { ...editableCapabilities, can_plan: false },
    connectionSourceId: endpointId("incident-commander"),
  });
  assert.equal(cannotPlan?.management.policy.mode, "read_only");
});

test("suppressed declared edge renders as reconnectable rather than a fresh connection", () => {
  const value = query({
    edges: [{
      edge: {
        a: { identity: "incident-commander" },
        b: { identity: "payments-sre" },
      },
      actual: false,
      declared: true,
      operator_added: false,
      suppressed: true,
      desired: false,
    }],
  });
  const normalized = normalizeConsoleTopologyQuery(value, {
    capabilities: editableCapabilities,
    connectionSourceId: endpointId("incident-commander"),
  });
  const edge = normalized?.management.affordances.find((entry) =>
    entry.edge.from === endpointId("incident-commander")
    && entry.edge.to === endpointId("payments-sre")
  );
  assert.equal(edge?.state, "degraded");
  assert.equal(edge?.preferredAction, "reconnect");
  assert.match(edge?.message || "", /Reconnect restores/);
  assert.equal(edge?.actions.reconnect?.state, "allowed");
});

test("actual suppressed conflict retries disconnect and pending receipts stay pending", () => {
  const value = query({
    edges: [{
      edge: {
        a: { identity: "incident-commander" },
        b: { identity: "payments-sre" },
      },
      actual: true,
      declared: true,
      operator_added: false,
      suppressed: true,
      desired: false,
    }],
  });
  const normalized = normalizeConsoleTopologyQuery(value, {
    capabilities: editableCapabilities,
    connectionSourceId: endpointId("incident-commander"),
  });
  const edge = normalized?.management.affordances.find((entry) =>
    entry.edge.from === endpointId("incident-commander")
    && entry.edge.to === endpointId("payments-sre")
  );
  assert.equal(edge?.state, "conflict");
  assert.equal(edge?.preferredAction, "disconnect");

  const pending = normalizeTopologyOperationReceipt({
    operation_id: "operation-pending",
    status: "pending",
    revision: 7,
    created_at: "2026-07-15T08:00:00Z",
    results: [{
      action: "disconnect",
      edge: {
        a: { authority: AUTHORITY, identity: "incident-commander" },
        b: { authority: AUTHORITY, identity: "payments-sre" },
      },
      status: "pending",
    }],
  });
  assert.equal(pending?.status, "running");
});

test("topology request and receipt adapters preserve stable identities and revisions", () => {
  const intent = {
    action: "disconnect" as const,
    edge: {
      from: endpointId("payments-sre"),
      to: endpointId("incident-commander"),
    },
    expectedRevision: 7,
    origin: "picker" as const,
  };
  assert.deepEqual(topologyPlanParams(intent), {
    expected_revision: 7,
    operations: [{
      action: "disconnect",
      edge: {
        a: { authority: AUTHORITY, identity: "payments-sre" },
        b: { authority: AUTHORITY, identity: "incident-commander" },
      },
    }],
  });
  assert.equal(topologyApplyParams(intent, "topology-1").idempotency_key, "topology-1");
  assert.throws(
    () => topologyPlanParams({
      ...intent,
      expectedAuthorityRevisions: {
        "mob/alpha": 11,
        "mob/beta": 19,
      },
    }),
    /authority-local.*bilateral/i,
  );

  const receipt = normalizeTopologyOperationReceipt({
    operation_id: "operation-1",
    idempotency_key: "topology-1",
    actor: "console-operator",
    status: "applied",
    base_revision: 7,
    revision: 8,
    authority_revisions: {
      "mob/alpha": { base_revision: 11, revision: 12 },
      "mob/beta": { base_revision: 19, revision: 20 },
    },
    created_at: "2026-07-15T08:00:00Z",
    results: [{
      action: "disconnect",
      edge: {
        a: { authority: AUTHORITY, identity: "incident-commander" },
        b: { authority: AUTHORITY, identity: "payments-sre" },
      },
      status: "applied",
      actual_before: true,
      actual_after: false,
    }],
  });
  assert.deepEqual(receipt?.edge, {
    from: endpointId("incident-commander"),
    to: endpointId("payments-sre"),
  });
  assert.equal(receipt?.idempotencyKey, "topology-1");
  assert.equal(receipt?.action, "disconnect");
  assert.equal(receipt?.status, "succeeded");
  assert.equal(receipt?.revision, 8);
  assert.deepEqual(receipt?.authorityRevisions, {
    "mob/alpha": { before: 11, after: 12 },
    "mob/beta": { before: 19, after: 20 },
  });
});

test("authority-qualified endpoint ids round-trip without collisions", () => {
  const left = endpointId("shared|identity", "mob/alpha");
  const right = endpointId("shared|identity", "mob/beta");
  assert.notEqual(left, right);
  assert.deepEqual(parseConsoleTopologyEndpointKey(left), {
    authority: "mob/alpha",
    identity: "shared|identity",
  });

  const crossAuthority = normalizeConsoleTopologyQuery(query({
    authority: "mob/alpha",
    policy: {
      mode: "editable",
      allow_bulk: false,
      max_batch_size: 1,
      allow_cross_authority: true,
    },
    nodes: [
      {
        endpoint: { authority: "mob/alpha", identity: "shared|identity" },
        role: "lead",
        affordances: {
          can_connect: true,
          can_disconnect: true,
          can_reconnect: true,
          can_bulk: false,
          can_cross_authority: true,
        },
      },
      {
        endpoint: { authority: "mob/beta", identity: "shared|identity" },
        role: "peer",
        affordances: {
          can_connect: true,
          can_disconnect: true,
          can_reconnect: true,
          can_bulk: false,
          can_cross_authority: true,
        },
      },
    ],
    edges: [],
  }), {
    capabilities: { ...editableCapabilities, can_cross_authority: true },
    connectionSourceId: left,
  });
  assert.deepEqual(crossAuthority?.nodes.map((node) => node.identity), [left, right]);
  assert.equal(crossAuthority?.management.affordances[0]?.actions.connect?.state, "denied");
  assert.throws(
    () => topologyPlanParams({
      action: "connect",
      edge: { from: left, to: right },
      expectedRevision: 9,
      origin: "picker",
    }),
    /authority-local.*cross-authority/i,
  );
  assert.throws(
    () => topologyApplyParams({
      action: "connect",
      edge: { from: left, to: "legacy-unqualified-endpoint" },
      expectedRevision: 9,
      origin: "picker",
    }, "mixed-authority"),
    /authority-local.*ambiguously qualified/i,
  );
});

test("observed remote edges remain visible but fail closed without a remote node grant", () => {
  const local = endpointId("shared", "mob/alpha");
  const remote = endpointId("shared", "mob/beta");
  const normalized = normalizeConsoleTopologyQuery(query({
    authority: "mob/alpha",
    policy: {
      mode: "editable",
      allow_bulk: false,
      max_batch_size: 1,
      allow_cross_authority: true,
    },
    nodes: [{
      endpoint: { authority: "mob/alpha", identity: "shared" },
      role: "lead",
      affordances: {
        can_connect: true,
        can_disconnect: true,
        can_reconnect: true,
        can_bulk: false,
        can_cross_authority: true,
      },
    }],
    edges: [{
      edge: {
        a: { authority: "mob/alpha", identity: "shared" },
        b: { authority: "mob/beta", identity: "shared" },
      },
      actual: true,
      declared: false,
      operator_added: true,
      suppressed: false,
      desired: true,
    }],
  }), {
    capabilities: { ...editableCapabilities, can_cross_authority: true },
    connectionSourceId: local,
  });

  assert.deepEqual(normalized?.nodes.map((node) => node.identity), [local, remote]);
  const edge = normalized?.management.affordances[0];
  assert.equal(edge?.state, "connected");
  assert.equal(edge?.actions.disconnect?.state, "denied");
});

test("topology query keeps every reported pair with its flags, not only the wired ones", () => {
  const flags = { actual: false, declared: false, operator_added: false, suppressed: false, desired: false };
  const pair = (a: string, b: string, set: Partial<typeof flags>, authority?: string) => ({
    edge: { a: { identity: a }, b: authority ? { authority, identity: b } : { identity: b } },
    ...flags,
    ...set,
  });
  const normalized = normalizeConsoleTopologyQuery(query({
    edges: [
      pair("incident-commander", "payments-sre", { actual: true, declared: true, desired: true }),
      // Declared and wanted, but the members do not list each other: a degraded wiring.
      pair("incident-commander", "api-investigator", { declared: true, desired: true }),
      // Declared, then disconnected by an operator.
      pair("payments-sre", "api-investigator", { declared: true, suppressed: true }),
      // Added by an operator across teams: one runtime never attests the other half.
      pair("incident-commander", "billing", { operator_added: true, desired: true }, "billing-team"),
      // A reported pair without any flag says nothing.
      pair("payments-sre", "scribe", {}),
    ],
  }), { capabilities: editableCapabilities });
  assert.ok(normalized);
  assert.equal(normalized.authority, AUTHORITY);
  const link = (a: string, b: string, authority?: string) => normalized.links.find((candidate) => {
    const ends = [candidate.from, candidate.to];
    return ends.includes(endpointId(a)) && ends.includes(endpointId(b, authority));
  });
  assert.deepEqual(link("incident-commander", "payments-sre"), {
    from: endpointId("incident-commander"),
    to: endpointId("payments-sre"),
    actual: true,
    declared: true,
    desired: true,
    suppressed: false,
    operatorAdded: false,
    cross: false,
  });
  assert.equal(link("incident-commander", "api-investigator")?.actual, false);
  assert.equal(link("incident-commander", "api-investigator")?.desired, true);
  assert.equal(link("payments-sre", "api-investigator")?.suppressed, true);
  assert.equal(link("payments-sre", "api-investigator")?.desired, false);
  const cross = link("incident-commander", "billing", "billing-team");
  assert.equal(cross?.cross, true);
  assert.equal(cross?.operatorAdded, true);
  assert.equal(link("payments-sre", "scribe"), undefined);
  assert.equal(normalized.links.length, 4);
  // Only actual peers are wired.
  assert.deepEqual(
    normalized.nodes.find((node) => node.ref?.identity === "incident-commander")?.wired_to,
    [endpointId("payments-sre")],
  );
  // The other team's agent is a node of its own scope.
  const remote = normalized.nodes.find((node) => node.ref?.identity === "billing");
  assert.equal(remote?.presentation?.crossScope, true);
  assert.equal(remote?.presentation?.scopeLabel, "billing-team");
  assert.equal(normalized.nodes.find((node) => node.ref?.identity === "payments-sre")?.presentation, undefined);
});

test("terminal server receipts replace optimistic edge blockers", () => {
  const intent = {
    action: "connect" as const,
    edge: {
      from: endpointId("incident-commander"),
      to: endpointId("api-investigator"),
    },
    expectedRevision: 7,
    origin: "picker" as const,
  };
  const optimistic = pendingTopologyReceipt(intent, "topology-idempotency");
  const terminal = {
    ...optimistic,
    operationId: "runtime-operation",
    status: "succeeded" as const,
  };
  const merged = mergeTopologyOperationReceipt([optimistic], terminal);
  assert.deepEqual(merged, [terminal]);

  const terminalWithoutKey = { ...terminal, idempotencyKey: null };
  assert.deepEqual(
    mergeTopologyOperationReceipt([optimistic], terminalWithoutKey),
    [terminalWithoutKey],
  );
});

test("commit then dropped apply response resolves with the exact key and one physical mutation", async () => {
  const intent = {
    action: "reconnect" as const,
    edge: {
      from: endpointId("incident-commander"),
      to: endpointId("payments-sre"),
    },
    expectedRevision: 7,
    origin: "picker" as const,
    reason: "restore incident command channel",
  };
  const request = createConsoleTopologyMutationRequest(intent, "topology-stable-key");
  const committedByKey = new Map<string, Record<string, unknown>>();
  const calls: Array<{ operation: string; params: Record<string, unknown> }> = [];
  let physicalMutations = 0;

  const execute = async (operation: "plan" | "apply" | "operation_get", params: Record<string, unknown>) => {
    calls.push({ operation, params: structuredClone(params) });
    if (operation === "plan") return { base_revision: 7 };
    if (operation === "operation_get") {
      const receipt = [...committedByKey.values()].find((candidate) => (
        candidate.operation_id === params.operation_id
      ));
      assert.ok(receipt, "operation/get resolves the committed operation id");
      return receipt;
    }

    const key = String(params.idempotency_key || "");
    const existing = committedByKey.get(key);
    if (existing) return existing;
    physicalMutations += 1;
    const committed = {
      operation_id: "operation-stable",
      idempotency_key: key,
      status: "applied",
      base_revision: 7,
      revision: 8,
      created_at: "2026-07-15T12:00:00Z",
      results: [{
        action: "reconnect",
        edge: {
          a: { authority: AUTHORITY, identity: "incident-commander" },
          b: { authority: AUTHORITY, identity: "payments-sre" },
        },
        status: "applied",
        actual_before: false,
        actual_after: true,
      }],
    };
    committedByKey.set(key, committed);
    throw new Error("connection closed after server commit");
  };

  const ambiguous = await executeConsoleTopologyMutation(request, execute);
  assert.equal(ambiguous.receipt.status, "failed");
  assert.equal(ambiguous.receipt.retryMode, "resolve_ambiguous");
  assert.equal(ambiguous.receipt.retryable, true);
  assert.equal(ambiguous.receipt.idempotencyKey, "topology-stable-key");
  assert.deepEqual(ambiguous.receipt.request, request.intent);
  assert.equal(physicalMutations, 1);

  const resolved = await resolveAmbiguousConsoleTopologyMutation(ambiguous.receipt, execute);
  assert.equal(resolved.error, null);
  assert.equal(resolved.receipt.status, "succeeded");
  assert.equal(resolved.receipt.operationId, "operation-stable");
  assert.equal(resolved.receipt.idempotencyKey, "topology-stable-key");
  assert.equal(resolved.receipt.retryable, false);
  assert.equal(physicalMutations, 1, "same-key recovery cannot apply a second physical mutation");
  assert.equal(committedByKey.size, 1, "one idempotency key represents one logical mutation");
  assert.deepEqual(calls.map((call) => call.operation), ["plan", "apply", "apply", "operation_get"]);
  assert.deepEqual(calls[1]?.params, calls[2]?.params, "ambiguous recovery replays the exact apply request");
});

test("same-key operation-in-progress remains resolvable instead of becoming a stale retry", async () => {
  const request = createConsoleTopologyMutationRequest({
    action: "connect",
    edge: {
      from: endpointId("incident-commander"),
      to: endpointId("api-investigator"),
    },
    expectedRevision: 7,
    origin: "picker",
  }, "topology-in-progress-key");
  const ambiguous = {
    ...pendingTopologyReceipt(request.intent, request.idempotencyKey),
    status: "failed" as const,
    retryable: true,
    retryMode: "resolve_ambiguous" as const,
  };
  let applyCalls = 0;
  const stillRunning = await resolveAmbiguousConsoleTopologyMutation(ambiguous, async (operation) => {
    assert.equal(operation, "apply");
    applyCalls += 1;
    const error = new Error("operation is still in progress") as Error & {
      rpcError: Record<string, unknown>;
    };
    error.rpcError = {
      code: -32009,
      data: { kind: "topology_operation_in_progress" },
    };
    throw error;
  });

  assert.equal(applyCalls, 1);
  assert.equal(stillRunning.receipt.retryable, true);
  assert.equal(stillRunning.receipt.retryMode, "resolve_ambiguous");
  assert.match(stillRunning.receipt.message || "", /still in progress/i);
});

test("a definitive stale revision conflict is never silently retried or rebased", async () => {
  const request = createConsoleTopologyMutationRequest({
    action: "disconnect",
    edge: {
      from: endpointId("incident-commander"),
      to: endpointId("payments-sre"),
    },
    expectedRevision: 7,
    origin: "picker",
  }, "topology-conflict-key");
  let applyCalls = 0;
  const execute = async (operation: "plan" | "apply" | "operation_get") => {
    if (operation === "plan") return { base_revision: 7 };
    applyCalls += 1;
    const error = new Error("topology revision conflict: expected 7, actual 8") as Error & {
      rpcError: Record<string, unknown>;
    };
    error.rpcError = {
      code: -32009,
      data: {
        kind: "topology_revision_conflict",
        expected_revision: 7,
        actual_revision: 8,
      },
    };
    throw error;
  };

  const conflict = await executeConsoleTopologyMutation(request, execute);
  assert.equal(conflict.receipt.status, "conflict");
  assert.equal(conflict.receipt.retryable, true);
  assert.equal(conflict.receipt.retryMode, "revision_rebase");
  assert.equal(applyCalls, 1);

  const notRecovered = await resolveAmbiguousConsoleTopologyMutation(conflict.receipt, execute);
  assert.match(notRecovered.error || "", /no ambiguous outcome/i);
  assert.equal(applyCalls, 1, "generic Retry must not mint or apply a rebased request");
});

test("a topology node selects its roster agent from an endpoint key or a plain id", () => {
  const agents = [
    { identity: "incident-commander", member_id: "member-commander", agent_id: "agent-commander", label: "Incident Commander" },
    { identity: "payments-sre", member_id: "member-sre", agent_id: "agent-sre", label: "Payments SRE" },
  ];
  const commanderKey = consoleTopologyEndpointKey({ identity: "incident-commander" }, "incident-command-center");
  assert.equal(commanderKey, "mk1|incident-command-center|incident-commander");
  assert.equal(topologyNodeAgent(agents, commanderKey)?.label, "Incident Commander");
  assert.equal(topologyNodeAgent(agents, "mk1|incident-command-center|payments-sre")?.label, "Payments SRE");
  assert.equal(topologyNodeAgent(agents, "member-sre")?.label, "Payments SRE");
  assert.equal(topologyNodeAgent(agents, "mk1|incident-command-center|unknown") === undefined, true);
});

test("the topology's team is the mob the experience names, never the runtime id", () => {
  assert.equal(topologyTeamKey({ topology: { live_snapshot: { mob_id: "ops-team" } } }), "ops-team");
  assert.equal(topologyTeamKey({ topology: { live_snapshot: { mob_id: " ops-team " } } }), "ops-team");
  // A runtime id keys a runtime, and a shared console's panel runtimes key
  // its stores: neither names the topology's team, so the root reads "Team".
  const runtimeOnly = {
    runtime_id: "ops-team",
    panel_runtimes: [{ key: "runtime-ops", label: "ops-team", namespace: "ops", memory: false, workgraph: false }],
    topology: { live_snapshot: { nodes: [], node_count: 0 } },
  };
  assert.equal(topologyTeamKey(runtimeOnly), undefined);
  assert.equal(topologyTeamKey({ topology: { live_snapshot: { mob_id: "  " } } }), undefined);
  assert.equal(topologyTeamKey({ topology: {} }), undefined);
  assert.equal(topologyTeamKey({}), undefined);
  assert.equal(topologyTeamKey(null), undefined);
});

type DockTarget = ConsoleDockTarget & { memberId?: string; identity?: string };

const topologyTarget: DockTarget = { id: "topology", kind: "topology", title: "Topology" };
const memoryTarget: DockTarget = { id: "memory", kind: "memory", title: "Memory" };
const chatTarget = (identity: string): DockTarget => ({
  id: `member-${identity}`,
  kind: "agent-chat",
  memberId: `member-${identity}`,
  identity,
  title: identity,
});

/** A dock driven by the same reducer the console's dock controller runs. */
function topologyDock() {
  let next = 1;
  const options = {
    createPanelState: ({ target }: { target?: DockTarget | null }) => ({
      id: `pane-${next++}`,
      target: target || null,
      mode: "console" as const,
    }),
    createSplitId: () => `split-${next++}`,
    createTabId: () => `tab-${next++}`,
  };
  let state: ConsoleDockState<DockTarget> = createConsoleDockState<DockTarget>({ initialTarget: topologyTarget, ...options });
  const topologyPaneId = state.focusedPanelId || "";
  const apply = (actions: ConsoleDockAction<DockTarget>[]) => {
    for (const action of actions) state = applyConsoleDockAction(state, action, options);
  };
  return {
    topologyPaneId,
    apply,
    state: () => state,
    clickChip: (identity: string) => apply(topologyAgentChatActions(state, topologyPaneId, chatTarget(identity))),
    panes: () => {
      const tab = state.tabs.find((candidate) => candidate.id === state.activeTabId);
      return collectConsoleDockPanelIds(tab?.layout).map((id) => {
        const pane = state.panels.find((panel) => panel.id === id);
        return { id, kind: pane?.target?.kind || "empty", identity: pane?.target?.identity || null };
      });
    },
  };
}

test("three topology role chip clicks keep one Topology pane and one chat pane", () => {
  const dock = topologyDock();
  for (const identity of ["commander", "investigator", "scribe"]) {
    dock.clickChip(identity);
    const panes = dock.panes();
    assert.deepEqual(panes.map((pane) => pane.kind), ["topology", "agent-chat"], `after opening ${identity}`);
    assert.equal(panes[0].id, dock.topologyPaneId);
    assert.equal(panes[1].identity, identity);
    assert.equal(dock.state().focusedPanelId, panes[1].id, "the chat pane takes focus");
  }
});

test("a topology role chip focuses a pane that already shows the agent", () => {
  const dock = topologyDock();
  dock.clickChip("commander");
  const chatPaneId = dock.panes()[1].id;
  dock.apply([{ type: "focus_panel", panelId: dock.topologyPaneId }]);

  assert.deepEqual(
    topologyAgentChatActions(dock.state(), dock.topologyPaneId, chatTarget("commander")),
    [{ type: "focus_panel", panelId: chatPaneId }],
  );
  dock.clickChip("commander");
  assert.equal(dock.panes().length, 2);
  assert.equal(dock.state().focusedPanelId, chatPaneId);
});

test("a topology role chip never replaces another panel beside the map", () => {
  const dock = topologyDock();
  dock.apply([{ type: "open_target", target: memoryTarget, intent: "split_right" }]);
  assert.deepEqual(dock.panes().map((pane) => pane.kind), ["topology", "memory"]);

  dock.clickChip("commander");
  assert.deepEqual(dock.panes().map((pane) => pane.kind), ["topology", "agent-chat", "memory"]);
  dock.clickChip("investigator");
  dock.clickChip("scribe");
  const panes = dock.panes();
  assert.deepEqual(panes.map((pane) => pane.kind), ["topology", "agent-chat", "memory"]);
  assert.equal(panes[1].identity, "scribe");
});
