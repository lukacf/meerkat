/**
 * #306 (0.9.4): identity status and refusals read Core's authority.
 *
 * Core's mob machine and journal hold an identity's session and generation,
 * so the gateway reports Core's answers: three typed refusal codes, the held
 * wakes an operator must clear, a view state of `active`, `dormant` or
 * `broken`, the refused wake that explains it, and Core's convergence status
 * for an identity a host intent declaration manages. These tests pin the SDK
 * side of that wire.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import * as sdk from "../src/index.js";
import { Meta } from "../src/builder.js";
import {
  IDENTITY_HISTORY_UNAVAILABLE_CODE,
  IDENTITY_HOST_ADOPTED_CODE,
  IDENTITY_UNRESUMABLE_CODE,
  IdentityHistoryUnavailableError,
  IdentityHostAdoptedError,
  IdentityUnresumableError,
  IdentityWakeHeldError,
  IDENTITY_WAKE_HELD_KINDS,
  RpcError,
  STORAGE_RESOLUTION_CODE,
  StorageResolutionError,
} from "../src/errors.js";
import { MetaRuntime } from "../src/runtime.js";
import {
  identityInspectionToDict,
  parseIdentityBootstrapStatus,
  parseIdentityInspection,
  parseIdentityRestoreProgress,
  parseIdentityStatus,
  parseMemberHealth,
} from "../src/types.js";

const CORE_CONVERGENCE = {
  agent_identity: "triage:main",
  desired_intent_revision: 4,
  active_intent_revision: 3,
  decision: "ensure_member_materialization",
  condition: "reconciling",
  observed_at_ms: 1_760_000_000_000,
  detail: null,
};

/** A runtime whose transport answers every request with `error`. */
function refusingRuntime(error: Record<string, unknown>): {
  _rpc(method: string, params: unknown): Promise<unknown>;
} {
  const rt = new MetaRuntime(
    (Meta.builder() as unknown as { _config: never })._config,
  ) as unknown as Record<string, unknown>;
  rt._running = true;
  rt._transport = {
    sendAsync: async (request: Record<string, unknown>) => ({
      jsonrpc: "2.0",
      id: request.id,
      error,
    }),
  };
  return rt as unknown as { _rpc(method: string, params: unknown): Promise<unknown> };
}

async function refusal(error: Record<string, unknown>): Promise<unknown> {
  try {
    await refusingRuntime(error)._rpc("meta/send", {});
  } catch (caught) {
    return caught;
  }
  throw new Error("the refusal was not raised");
}

describe("#306: identity refusals carry Core's typed answers", () => {
  it("uses the Core contract codes and exports them", () => {
    assert.equal(IDENTITY_UNRESUMABLE_CODE, -32051);
    assert.equal(IDENTITY_HISTORY_UNAVAILABLE_CODE, -32052);
    assert.equal(IDENTITY_HOST_ADOPTED_CODE, -32053);
    for (const name of [
      "IDENTITY_UNRESUMABLE_CODE",
      "IDENTITY_HISTORY_UNAVAILABLE_CODE",
      "IDENTITY_HOST_ADOPTED_CODE",
      "IdentityUnresumableError",
      "IdentityHistoryUnavailableError",
      "IdentityHostAdoptedError",
      "IdentityWakeHeldError",
      "IDENTITY_WAKE_HELD_KINDS",
      "parseWakeRefusal",
      "parseIdentityConvergence",
    ]) {
      assert.ok(name in sdk, name);
    }
  });

  it("maps an unresumable identity to a typed final error", async () => {
    const err = await refusal({
      code: -32051,
      message: "refused",
      data: {
        kind: "identity_unresumable",
        identity: "triage:main",
        session_id: "s-1",
        reason: "archived_not_revivable",
        remedy: "delete_identity",
        retryable: false,
      },
    });
    assert.ok(err instanceof IdentityUnresumableError);
    assert.ok(err instanceof RpcError);
    assert.equal(err.code, -32051);
    assert.equal(err.retryable, false);
    assert.deepEqual(
      [err.identity, err.sessionId, err.reason, err.remedy, err.method],
      ["triage:main", "s-1", "archived_not_revivable", "delete_identity", "meta/send"],
    );
  });

  for (const [data, kind, sessionId] of [
    [
      { kind: "identity_history_unavailable", identity: "w", retryable: true },
      "identity_history_unavailable",
      null,
    ],
    [
      { kind: "identity_wake_retryable", identity: "w", session_id: "s-2", retryable: true },
      "identity_wake_retryable",
      "s-2",
    ],
    [
      { kind: "identity_wake_retryable", identity: "w", session_id: null, retryable: true },
      "identity_wake_retryable",
      null,
    ],
    [
      { kind: "identity_session_absent", identity: "w", session_id: "s-3", retryable: true },
      "identity_session_absent",
      "s-3",
    ],
  ] as const) {
    it(`maps the retryable ${kind} kind (session ${sessionId}) to one typed error`, async () => {
      const err = await refusal({ code: -32052, message: "refused", data });
      assert.ok(err instanceof IdentityHistoryUnavailableError);
      assert.equal(err.retryable, true);
      assert.deepEqual([err.kind, err.identity, err.sessionId], [kind, "w", sessionId]);
    });
  }

  it("maps a roster-wide history failure that names no identity", async () => {
    // Reading the whole roster's history failed; no one identity is named.
    const err = await refusal({
      code: -32052,
      message: "refused",
      data: { kind: "identity_history_unavailable", retryable: true },
    });
    assert.ok(err instanceof IdentityHistoryUnavailableError);
    assert.equal(err.retryable, true);
    assert.deepEqual(
      [err.kind, err.identity, err.sessionId],
      ["identity_history_unavailable", null, null],
    );
  });

  it("maps a host-adopted identity to a typed error", async () => {
    const err = await refusal({
      code: -32053,
      message: "refused",
      data: { kind: "identity_host_adopted", identity: "w", retryable: false },
    });
    assert.ok(err instanceof IdentityHostAdoptedError);
    assert.equal(err.retryable, false);
    assert.equal(err.identity, "w");
  });

  for (const code of [-32051, -32052, -32053]) {
    it(`keeps ${code} typed without refusal data`, async () => {
      const err = await refusal({ code, message: "refused" });
      assert.ok(err instanceof RpcError);
      assert.notEqual(Object.getPrototypeOf(err), RpcError.prototype);
      assert.equal(err.code, code);
      assert.equal((err as unknown as { identity: unknown }).identity, null);
    });
  }
});

describe("#306: held wakes are typed", () => {
  for (const [data, hold, reason, operatorPath] of [
    [
      {
        kind: "mob_member_session_repair_required",
        durable_resume_hold: "audited_endpoint_divergence",
        session_id: "s-1",
        content_retained: true,
        retryable: false,
        operator_path: ["session repair-wholeblob", "meta/reload_member"],
      },
      "audited_endpoint_divergence",
      null,
      ["session repair-wholeblob", "meta/reload_member"],
    ],
    [
      {
        kind: "mob_member_session_resume_held",
        durable_resume_hold: "tail_held_for_recovery",
        session_id: "s-1",
        content_retained: true,
        retryable: false,
      },
      "tail_held_for_recovery",
      null,
      [],
    ],
    [
      {
        kind: "mob_member_session_resume_held",
        resume_unavailable_reason: "committed_boundary_unprovable",
        session_id: "s-1",
        content_retained: true,
        retryable: false,
      },
      null,
      "committed_boundary_unprovable",
      [],
    ],
    [
      {
        kind: "identity_role_migration_refused",
        identity: "w",
        session_id: "s-1",
        content_retained: true,
        retryable: false,
      },
      null,
      null,
      [],
    ],
  ] as const) {
    it(`maps the held ${data.kind} wake (${hold ?? reason}) to a typed error`, async () => {
      const err = await refusal({ code: -32603, message: "held", data });
      assert.ok(err instanceof IdentityWakeHeldError);
      assert.equal(err.code, -32603);
      assert.equal(err.retryable, false);
      assert.deepEqual(
        [err.kind, err.sessionId, err.durableResumeHold],
        [data.kind, "s-1", hold],
      );
      assert.equal(err.resumeUnavailableReason, reason);
      assert.deepEqual(err.operatorPath, operatorPath);
      assert.ok(IDENTITY_WAKE_HELD_KINDS.has(data.kind));
    });
  }

  for (const data of [undefined, { kind: "actor_terminated", operation: "send" }, { no_kind: true }]) {
    it(`keeps other internal errors plain (${JSON.stringify(data)})`, async () => {
      const err = await refusal(
        data === undefined
          ? { code: -32603, message: "internal" }
          : { code: -32603, message: "internal", data },
      );
      assert.ok(err instanceof RpcError);
      assert.equal(Object.getPrototypeOf(err), RpcError.prototype);
    });
  }

  it("keeps an owed held completion a plain RpcError carrying its data", async () => {
    // An identity a held pass did not publish owes its host a held
    // completion: an admission still owed, not a held wake.
    const data = { kind: "held_completion_owed", identity: "w" };
    const err = await refusal({ code: -32603, message: "owed", data });
    assert.ok(err instanceof RpcError);
    assert.equal(Object.getPrototypeOf(err), RpcError.prototype);
    assert.equal(err.code, -32603);
    assert.deepEqual(err.data, data);
    assert.ok(!IDENTITY_WAKE_HELD_KINDS.has(data.kind));
    assert.deepEqual(
      parseIdentityRestoreProgress({
        stage: "refused",
        code: -32603,
        kind: "held_completion_owed",
      }),
      { stage: "refused", code: -32603, kind: "held_completion_owed" },
    );
  });
});

describe("#306: identity status reads Core's view", () => {
  it("carries the final wake refusal of a broken identity", () => {
    const status = parseIdentityStatus({
      identity: "triage:main",
      state: "broken",
      agent_runtime_id: null,
      session_id: "s-1",
      generation: 3,
      profile: "triage",
      addressability: "addressable",
      labels: {},
      intent_managed: false,
      wake_refusal: { code: -32051, session_id: "s-1", remedy: "delete_identity" },
    });
    assert.equal(status.lifecycleState, "broken");
    assert.equal(status.agentRuntimeId, null);
    assert.deepEqual([status.sessionId, status.generation], ["s-1", 3]);
    assert.equal(status.intentManaged, false);
    assert.equal(status.convergence, null);
    assert.deepEqual(status.wakeRefusal, {
      code: -32051,
      sessionId: "s-1",
      remedy: "delete_identity",
    });
  });

  it("carries Core's convergence for an intent-managed identity", () => {
    const status = parseIdentityStatus({
      identity: "triage:main",
      state: "active",
      agent_runtime_id: "rt-1",
      session_id: "s-1",
      generation: 7,
      intent_managed: true,
      convergence: CORE_CONVERGENCE,
      wake_refusal: null,
    });
    assert.equal(status.intentManaged, true);
    assert.equal(status.wakeRefusal, null);
    assert.deepEqual(status.convergence, {
      agentIdentity: "triage:main",
      desiredIntentRevision: 4,
      activeIntentRevision: 3,
      decision: "ensure_member_materialization",
      condition: "reconciling",
      observedAtMs: 1_760_000_000_000,
      detail: null,
    });
  });

  it("reads absent and null optional keys as null", () => {
    for (const wire of [
      { identity: "w", state: "dormant" },
      {
        identity: "w",
        state: "dormant",
        agent_runtime_id: null,
        session_id: null,
        generation: null,
        convergence: null,
        wake_refusal: null,
      },
    ]) {
      const status = parseIdentityStatus(wire);
      assert.deepEqual(
        [status.agentRuntimeId, status.sessionId, status.generation],
        [null, null, null],
      );
      assert.deepEqual(
        [status.intentManaged, status.convergence, status.wakeRefusal],
        [false, null, null],
      );
    }
  });

  it("keeps newer remedy and state values as sent", () => {
    const status = parseIdentityStatus({
      identity: "w",
      state: "a_future_state",
      wake_refusal: { code: -32603, session_id: "s", remedy: "a_future_remedy" },
    });
    assert.equal(status.lifecycleState, "a_future_state");
    assert.deepEqual(
      [status.wakeRefusal?.code, status.wakeRefusal?.remedy],
      [-32603, "a_future_remedy"],
    );
  });

  it("reports the runtime mode and pending customizer tools", () => {
    const status = parseIdentityStatus({
      identity: "w",
      state: "active",
      runtime_mode: "turn_driven",
      customizer_tools_pending: { reason: "customize_build failed" },
    });
    assert.equal(status.runtimeMode, "turn_driven");
    assert.deepEqual(status.customizerToolsPending, { reason: "customize_build failed" });
    const bare = parseIdentityStatus({ identity: "w", state: "active" });
    assert.deepEqual([bare.runtimeMode, bare.customizerToolsPending], [null, null]);
  });

  it("drops the Meta-minted continuity facts", () => {
    const status = parseIdentityStatus({
      identity: "w",
      state: "active",
      checkpoint_version: 5,
      continuity_unrecoverable: { reason: "x" },
      session_repair_required: { commands: [] },
    }) as unknown as Record<string, unknown>;
    for (const key of [
      "checkpointVersion",
      "continuityUnrecoverable",
      "sessionRepairRequired",
      "checkpoint_version",
    ]) {
      assert.ok(!(key in status), key);
    }
  });
});

describe("#306: identity view parts", () => {
  it("names the session a fresh wake replaced", () => {
    const status = parseIdentityStatus({
      identity: "w",
      state: "active",
      agent_runtime_id: "rt-2",
      session_id: "s-2",
      generation: 1,
      intent_managed: false,
      replaced_session: { session_id: "s-1", reason: "absent" },
    });
    assert.deepEqual(status.replacedSession, { sessionId: "s-1", reason: "absent" });
    const later = parseIdentityStatus({
      identity: "w",
      state: "active",
      replaced_session: { session_id: "s-1", reason: "a_future_reason" },
    });
    assert.equal(later.replacedSession?.reason, "a_future_reason");
    assert.equal(parseIdentityStatus({ identity: "w", state: "active" }).replacedSession, null);
  });

  it("carries Core's member id of the last binding", () => {
    const status = parseIdentityStatus({
      identity: "agent:w",
      state: "active",
      session_id: "s-2",
      member_id: "mk--agent_cw",
      intent_managed: false,
    });
    assert.equal(status.memberId, "mk--agent_cw");
    assert.equal(parseIdentityStatus({ identity: "agent:w", state: "dormant" }).memberId, null);
  });

  for (const remedy of [
    "delete_identity",
    "repair_session",
    "resolve_role_migration",
    "operator_review",
  ]) {
    it(`reads the ${remedy} remedy as sent`, () => {
      const status = parseIdentityStatus({
        identity: "w",
        state: "dormant",
        wake_refusal: { code: -32603, session_id: "s", remedy },
      });
      assert.deepEqual(status.wakeRefusal, { code: -32603, sessionId: "s", remedy });
    });
  }

  it("inspection carries the identity view atoms", () => {
    const wire = {
      identity: "triage:main",
      state: "dormant",
      agent_runtime_id: null,
      session_id: "s-1",
      generation: 2,
      intent_managed: true,
      convergence: CORE_CONVERGENCE,
      wake_refusal: { code: -32603, session_id: "s-1", remedy: "operator_review" },
      replaced_session: { session_id: "s-0", reason: "absent" },
      is_final: false,
      peer_reachable_count: 0,
    };
    const inspection = parseIdentityInspection(wire);
    assert.deepEqual([inspection.agentRuntimeId, inspection.sessionId], [null, "s-1"]);
    assert.equal(inspection.intentManaged, true);
    assert.equal(inspection.convergence?.condition, "reconciling");
    assert.deepEqual(inspection.wakeRefusal, {
      code: -32603,
      sessionId: "s-1",
      remedy: "operator_review",
    });
    assert.deepEqual(inspection.replacedSession, { sessionId: "s-0", reason: "absent" });
    const out = identityInspectionToDict(inspection);
    assert.equal(out.session_id, "s-1");
    assert.ok(!("agent_runtime_id" in out));
    assert.equal(out.intent_managed, true);
    for (const key of ["convergence", "wake_refusal", "replaced_session"] as const) {
      assert.deepEqual(out[key], wire[key], key);
    }
  });

  it("inspection reads an older frame without its continuity facts", () => {
    const inspection = parseIdentityInspection({
      identity: "w",
      checkpoint_version: 4,
      continuity_health: "healthy",
      lease_healthy: true,
      lease: { holder: "h" },
      continuity: { session_id: "s-0" },
    });
    assert.deepEqual([inspection.agentRuntimeId, inspection.sessionId], [null, null]);
    assert.equal(inspection.intentManaged, false);
    assert.deepEqual(
      [inspection.convergence, inspection.wakeRefusal, inspection.replacedSession],
      [null, null, null],
    );
    const record = inspection as unknown as Record<string, unknown>;
    const out = identityInspectionToDict(inspection);
    for (const key of [
      "checkpointVersion",
      "continuityHealth",
      "leaseHealthy",
      "lease",
      "continuity",
    ]) {
      assert.ok(!(key in record), key);
    }
    for (const key of [
      "checkpoint_version",
      "continuity_health",
      "lease_healthy",
      "lease",
      "continuity",
    ]) {
      assert.ok(!(key in out), key);
    }
  });

  it("restore progress reports a refused restore with its code", () => {
    assert.deepEqual(
      parseIdentityRestoreProgress({
        stage: "refused",
        code: -32051,
        kind: "identity_unresumable",
      }),
      { stage: "refused", code: -32051, kind: "identity_unresumable" },
    );
    assert.deepEqual(
      parseIdentityRestoreProgress({
        stage: "refused",
        code: -32603,
        kind: "mob_member_session_resume_held",
      }),
      { stage: "refused", code: -32603, kind: "mob_member_session_resume_held" },
    );
    assert.deepEqual(parseIdentityRestoreProgress({ stage: "refused", code: -32052 }), {
      stage: "refused",
      code: -32052,
      kind: null,
    });
  });

  it("restore progress keeps other stages without a code", () => {
    for (const stage of ["registered", "resuming", "minted", "resumed", "a_future_stage"]) {
      assert.deepEqual(parseIdentityRestoreProgress({ stage }), {
        stage,
        code: null,
        kind: null,
      });
    }
  });
});

describe("#306: member health reads Core's view", () => {
  it("carries the last wake refusal", () => {
    const health = parseMemberHealth({
      identity: "w",
      state: "dormant",
      generation: 4,
      wake_refusal: { code: -32603, session_id: "s-1", remedy: "repair_session" },
      session_repair_required: { commands: ["repair"] },
    });
    assert.deepEqual(health.wakeRefusal, {
      code: -32603,
      sessionId: "s-1",
      remedy: "repair_session",
    });
    assert.equal(health.generation, 4);
    // An older frame still carrying the continuity-era park reads without it.
    const older = parseMemberHealth({
      identity: "w",
      state: "active",
      continuity_unrecoverable: { reason: "x" },
    });
    assert.equal(older.wakeRefusal, null);
    assert.ok(!("continuityUnrecoverable" in (older as unknown as Record<string, unknown>)));
  });
});

describe("#306: the first boot after storage-migrate-native", () => {
  const bootstrapWire = (extra: Record<string, unknown> = {}) => ({
    mode: { mode: "lazy_materialize" },
    complete: false,
    ready: false,
    counts: {},
    identities: {},
    ...extra,
  });

  it("lists the pending first-boot rows on the bootstrap status", () => {
    const status = parseIdentityBootstrapStatus(
      bootstrapWire({
        first_boot_pending: [
          { identity: "triage:main", kind: "resume", waiting_for: "carry_out" },
          { identity: "triage:aux", kind: "fresh", waiting_for: "roster" },
          { identity: "triage:new", kind: "a_future_kind", waiting_for: "a_future_wait" },
          { identity: 7 },
        ],
      }),
    );
    assert.deepEqual(status.firstBootPending, [
      { identity: "triage:main", kind: "resume", waitingFor: "carry_out" },
      { identity: "triage:aux", kind: "fresh", waitingFor: "roster" },
      // Newer kinds and waits are kept as sent.
      { identity: "triage:new", kind: "a_future_kind", waitingFor: "a_future_wait" },
      // A malformed row is still counted.
      { identity: "", kind: "unknown", waitingFor: "unknown" },
    ]);
    assert.equal(typeof sdk.parseFirstBootPendingRow, "function");
  });

  it("has no pending rows when the gateway sends none", () => {
    assert.deepEqual(parseIdentityBootstrapStatus(bootstrapWire()).firstBootPending, []);
  });

  it("lists a candidate's deferred identities, which are not broken", () => {
    const status = parseIdentityBootstrapStatus(
      bootstrapWire({
        complete: true,
        counts: { active: 1, broken: 0, deferred: 4 },
        deferred: [
          { identity: "agent:dormant", reason: "resume_member" },
          { identity: "agent:lead", reason: "create_member" },
          { identity: "agent:specialist", reason: "move_role" },
          { identity: "agent:next", reason: "a_future_reason" },
        ],
      }),
    );
    assert.deepEqual(status.deferred, [
      { identity: "agent:dormant", reason: "resume_member" },
      { identity: "agent:lead", reason: "create_member" },
      { identity: "agent:specialist", reason: "move_role" },
      // Newer reasons are kept as sent.
      { identity: "agent:next", reason: "a_future_reason" },
    ]);
    assert.equal(status.counts.deferred, 4);
    assert.equal(status.counts.broken, 0);
    assert.equal(typeof sdk.parseDeferredIdentity, "function");
    // The gateway omits both when nothing is deferred.
    assert.deepEqual(parseIdentityBootstrapStatus(bootstrapWire()).deferred, []);
  });

  for (const [data, fields] of [
    [
      {
        kind: "first_boot_manifest_unreadable",
        path: "/state/storage-migrate-native.first-boot.json",
        retryable: false,
      },
      [
        "first_boot_manifest_unreadable",
        null,
        null,
        null,
        null,
        null,
        "/state/storage-migrate-native.first-boot.json",
        null,
        null,
        false,
      ],
    ],
    [
      {
        kind: "first_boot_row_refused",
        identity: "w",
        row_kind: "resume",
        manifest_session: "s-1",
        core_session: "s-2",
        retryable: false,
      },
      ["first_boot_row_refused", "w", "resume", "s-1", "s-2", null, null, null, null, false],
    ],
    [
      {
        kind: "first_boot_row_refused",
        identity: "w",
        row_kind: "fresh",
        manifest_session: "s-1",
        core_session: null,
        retryable: false,
      },
      ["first_boot_row_refused", "w", "fresh", "s-1", null, null, null, null, null, false],
    ],
    [
      {
        kind: "first_boot_row_pending",
        identity: "w",
        row_kind: "resume",
        waiting_for: "roster",
        operation: "delete_identity",
        remedy: "add_to_roster",
        retryable: true,
      },
      [
        "first_boot_row_pending",
        "w",
        "resume",
        null,
        null,
        "delete_identity",
        null,
        "roster",
        "add_to_roster",
        true,
      ],
    ],
    [
      {
        kind: "first_boot_row_pending",
        identity: "w",
        row_kind: "fresh",
        waiting_for: "carry_out",
        operation: "reset",
        remedy: "wake_identity",
        retryable: true,
      },
      [
        "first_boot_row_pending",
        "w",
        "fresh",
        null,
        null,
        "reset",
        null,
        "carry_out",
        "wake_identity",
        true,
      ],
    ],
  ] as const) {
    it(`types the ${data.kind} refusal (${"row_kind" in data ? data.row_kind : "no row"})`, async () => {
      const err = await refusal({ code: STORAGE_RESOLUTION_CODE, message: "refused", data });
      assert.ok(err instanceof StorageResolutionError);
      assert.deepEqual(
        [
          err.kind,
          err.identity,
          err.rowKind,
          err.manifestSession,
          err.coreSession,
          err.operation,
          err.path,
          err.waitingFor,
          err.remedy,
          err.retryable,
        ],
        fields,
      );
    });
  }

  it("reads a storage refusal without data as not retryable", async () => {
    const err = await refusal({ code: STORAGE_RESOLUTION_CODE, message: "refused" });
    assert.ok(err instanceof StorageResolutionError);
    assert.deepEqual([err.kind, err.identity, err.retryable], [null, null, false]);
  });
});
