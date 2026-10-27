import assert from "node:assert/strict";
import test from "node:test";

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
  topologyPlanParams,
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
