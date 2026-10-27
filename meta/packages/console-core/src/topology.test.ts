import assert from "node:assert/strict";
import test from "node:test";

import {
  canonicalTopologyEdge,
  topologyAuthorityRevisionToken,
  topologyCapabilityAllowsRequest,
  topologyEdgeFromKey,
  topologyEdgeKey,
  topologyIsBilateralAuthorityRevisionMap,
  topologyMutationIntent,
  topologyOperationFor,
  topologyOperationIsPending,
  type TopologyManagementState,
} from "./topology";

const editable: TopologyManagementState = {
  revision: 7,
  policy: {
    mode: "editable",
    capabilities: {
      connect: { state: "allowed" },
      disconnect: { state: "allowed" },
      reconnect: { state: "approval_required" },
    },
  },
  affordances: [{
    edge: { from: "beta", to: "alpha" },
    state: "degraded",
    actions: {
      reconnect: { state: "approval_required", reason: "Review required" },
      disconnect: { state: "denied", reason: "Protected link" },
    },
  }],
};

test("canonical edge identity is order independent", () => {
  assert.equal(topologyEdgeKey("alpha", "beta"), '["alpha","beta"]');
  assert.equal(topologyEdgeKey({ from: "beta", to: "alpha" }), '["alpha","beta"]');
  assert.deepEqual(canonicalTopologyEdge({ from: "beta", to: "alpha" }), {
    from: "alpha",
    to: "beta",
  });
  const opaque = topologyEdgeKey("mk1|a|same", "mk1|b|same");
  assert.deepEqual(topologyEdgeFromKey(opaque), {
    from: "mk1|a|same",
    to: "mk1|b|same",
  });
  assert.equal(topologyEdgeFromKey("alpha|beta"), null);
});

test("capabilities distinguish actionable approval from denied states", () => {
  assert.equal(topologyCapabilityAllowsRequest({ state: "allowed" }), true);
  assert.equal(topologyCapabilityAllowsRequest({ state: "approval_required" }), true);
  assert.equal(topologyCapabilityAllowsRequest({ state: "denied" }), false);
  assert.equal(topologyCapabilityAllowsRequest(undefined), false);
});

test("mutation intents require explicit global and per-edge authority", () => {
  assert.deepEqual(
    topologyMutationIntent(editable, "reconnect", { from: "alpha", to: "beta" }, "picker"),
    {
      action: "reconnect",
      edge: { from: "alpha", to: "beta" },
      expectedRevision: 7,
      origin: "picker",
    },
  );
  assert.equal(
    topologyMutationIntent(editable, "disconnect", { from: "alpha", to: "beta" }, "picker"),
    null,
  );
  assert.equal(
    topologyMutationIntent(
      { ...editable, policy: { ...editable.policy, mode: "read_only" } },
      "reconnect",
      { from: "alpha", to: "beta" },
      "picker",
    ),
    null,
  );
});

test("bilateral mutation intents preserve the exact authority CAS map", () => {
  const crossEdge = {
    from: "mk1|mob%2Falpha|shared",
    to: "mk1|mob%2Fbeta|shared",
  };
  const bilateral: TopologyManagementState = {
    ...editable,
    affordances: [{
      edge: crossEdge,
      state: "degraded",
      actions: { reconnect: { state: "allowed" } },
    }],
    authorityRevisions: {
      "mob/alpha": 11,
      "mob/beta": 19,
    },
  };
  assert.deepEqual(
    topologyMutationIntent(
      bilateral,
      "reconnect",
      crossEdge,
      "picker",
    ),
    {
      action: "reconnect",
      edge: crossEdge,
      expectedRevision: 7,
      expectedAuthorityRevisions: {
        "mob/alpha": 11,
        "mob/beta": 19,
      },
      origin: "picker",
    },
  );
  assert.equal(
    topologyAuthorityRevisionToken({ "mob/beta": 19, "mob/alpha": 11 }),
    '[["mob/alpha",11],["mob/beta",19]]',
  );
});

test("mixed topology views copy pair-specific CAS without crossing authority pairs", () => {
  const alphaBeta = { from: "alpha", to: "beta" };
  const alphaGamma = { from: "alpha", to: "gamma" };
  const mixed: TopologyManagementState = {
    revision: "aggregate-render-token",
    casScope: "mixed",
    // This legacy map must never be used by a mixed view.
    authorityRevisions: { wrongA: 90, wrongB: 91 },
    policy: {
      mode: "editable",
      capabilities: {
        connect: { state: "allowed" },
        disconnect: { state: "allowed" },
        reconnect: { state: "allowed" },
      },
    },
    affordances: [
      {
        edge: alphaBeta,
        state: "disconnected",
        expectedRevision: "alpha-beta",
        expectedAuthorityRevisions: { alphaAuthority: 4, betaAuthority: 8 },
        actions: { connect: { state: "allowed" } },
      },
      {
        edge: alphaGamma,
        state: "degraded",
        expectedRevision: "alpha-gamma",
        expectedAuthorityRevisions: { alphaAuthority: 4, gammaAuthority: 12 },
        actions: { reconnect: { state: "allowed" } },
      },
    ],
  };

  assert.deepEqual(topologyMutationIntent(mixed, "connect", alphaBeta, "graph"), {
    action: "connect",
    edge: alphaBeta,
    expectedRevision: "alpha-beta",
    expectedAuthorityRevisions: { alphaAuthority: 4, betaAuthority: 8 },
    origin: "graph",
  });
  assert.deepEqual(topologyMutationIntent(mixed, "reconnect", alphaGamma, "picker"), {
    action: "reconnect",
    edge: alphaGamma,
    expectedRevision: "alpha-gamma",
    expectedAuthorityRevisions: { alphaAuthority: 4, gammaAuthority: 12 },
    origin: "picker",
  });
});

test("mixed topology CAS fails closed when an edge map is absent or malformed", () => {
  const edge = { from: "alpha", to: "beta" };
  const management: TopologyManagementState = {
    revision: 3,
    casScope: "mixed",
    authorityRevisions: { globalA: 1, globalB: 2 },
    policy: {
      mode: "editable",
      capabilities: {
        connect: { state: "allowed" },
        disconnect: { state: "allowed" },
        reconnect: { state: "allowed" },
      },
    },
    affordances: [{
      edge,
      state: "disconnected",
      actions: { connect: { state: "allowed" } },
    }],
  };
  assert.equal(topologyMutationIntent(management, "connect", edge, "graph"), null);
  assert.equal(topologyIsBilateralAuthorityRevisionMap({ alpha: 1 }), false);
  assert.equal(topologyIsBilateralAuthorityRevisionMap({ alpha: 1, beta: Number.NaN }), false);
  assert.equal(topologyIsBilateralAuthorityRevisionMap({ alpha: 1, beta: -1 }), false);
  assert.equal(topologyIsBilateralAuthorityRevisionMap({ alpha: 1, beta: 2 }), true);

  management.affordances[0].expectedAuthorityRevisions = { alpha: 1 };
  assert.equal(topologyMutationIntent(management, "connect", edge, "graph"), null);
});

test("pending operations fail closed and the latest receipt wins", () => {
  const management: TopologyManagementState = {
    ...editable,
    operations: [
      {
        operationId: "old",
        action: "reconnect",
        edge: { from: "alpha", to: "beta" },
        status: "failed",
      },
      {
        operationId: "current",
        action: "reconnect",
        edge: { from: "beta", to: "alpha" },
        status: "pending_approval",
      },
    ],
  };
  const receipt = topologyOperationFor(management, { from: "alpha", to: "beta" });
  assert.equal(receipt?.operationId, "current");
  assert.equal(topologyOperationIsPending(receipt), true);
  assert.equal(
    topologyMutationIntent(management, "reconnect", { from: "alpha", to: "beta" }, "graph"),
    null,
  );
});
