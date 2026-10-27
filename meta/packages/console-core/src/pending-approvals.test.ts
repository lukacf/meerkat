import assert from "node:assert/strict";
import test from "node:test";
import { approvalMatchesConversation, createPendingApprovalResource, normalizePendingApproval, type ApprovalResourceEnvironment } from "./pending-approvals";
import { CONSOLE_COMMAND_NAMES, createMetaConsoleController as sharedController, type MetaConsoleTransport } from "./headless";
import { createMetaConsoleController as stockController } from "../../../console/src/lib/headless";
import { migrateConsoleWorkbenchTarget } from "./targets";
import { CONSOLE_RPC_METHODS } from "./contract";

const tick = async () => { for (let i = 0; i < 15; i++) await Promise.resolve(); };
function deferred<T>() { let resolve!: (value: T) => void; let reject!: (error: unknown) => void; const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
function clock() {
  let now = 0, visible = true, id = 0;
  const timers = new Map<number, { at: number; callback(): void }>();
  const visibility = new Set<() => void>();
  const environment: ApprovalResourceEnvironment = {
    now: () => now, visible: () => visible,
    setTimeout(callback, ms) { const key = ++id; timers.set(key, { at: now + ms, callback }); return key; },
    clearTimeout(key) { timers.delete(key as number); },
    onVisibilityChange(callback) { visibility.add(callback); return () => { visibility.delete(callback); }; },
  };
  return { environment, advance(ms: number) { const until = now + ms; while (true) { const next = Array.from(timers.entries()).filter(([, t]) => t.at <= until).sort((a, b) => a[1].at - b[1].at)[0]; if (!next) break; now = next[1].at; timers.delete(next[0]); next[1].callback(); } now = until; }, visible(value: boolean) { visible = value; for (const listener of visibility) listener(); }, timers };
}
const row = (id = "p1") => ({ pending_id: id, action_id: `action:${id}`, action: "Deploy service", actor_id: "actor", risk_tier: "r3", deadline_at_ms: 1000 });
const accepted = (id = "p1", decision = "approve") => ({ pending_id: id, action_id: `action:${id}`, approver_id: "operator", decision, outcome: decision === "escalate" ? "pending_approval" : "allowed", decided_at_ms: 10, ...(decision === "escalate" ? { next_pending_id: "p2" } : {}) });

test("pending approval projection keeps actor, requested approver and notification references distinct", () => {
  const request = normalizePendingApproval({ ...row(), requested_approver: "human:reviewer",
    approval_recipient: "review-queue", approval_channel: "notification",
    approval_route_id: "route:1", approval_delivery_id: "delivery:1" })!;
  assert.equal(request.actorId, "actor");
  assert.equal(request.requestedApprover, "human:reviewer");
  assert.equal(request.approvalRecipient, "review-queue");
  assert.equal(request.approvalChannel, "notification");
  assert.equal(request.approvalRouteId, "route:1");
  assert.equal(request.approvalDeliveryId, "delivery:1");
  const absent = normalizePendingApproval({ ...row(), requested_approver: 42,
    approval_recipient: {}, approval_channel: " ", approval_route_id: false, approval_delivery_id: [] })!;
  assert.equal(absent.requestedApprover, undefined);
  assert.equal(absent.approvalRecipient, undefined);
  assert.equal(absent.approvalChannel, undefined);
  assert.equal(absent.approvalRouteId, undefined);
  assert.equal(absent.approvalDeliveryId, undefined);
});

test("an escalated successor uses its own routing facts without reviving the previous approver or delivery", async () => {
  const c = clock();
  const first = { ...row(), requested_approver: "human:reviewer", approval_recipient: "review-queue",
    approval_channel: "notification", approval_route_id: "route:1", approval_delivery_id: "delivery:1" };
  const successor = { ...first, pending_id: "p2", requested_approver: null,
    approval_route_id: null, approval_delivery_id: null };
  let reads = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: async () => ({ pending: [++reads === 1 ? first : successor] }),
    decide: async () => accepted("p1", "escalate"),
  });
  try {
    await tick();
    assert.equal(resource.getSnapshot().requests[0].requestedApprover, "human:reviewer");
    await resource.decide("p1", "escalate");
    const request = resource.getSnapshot().requests[0];
    assert.equal(request.pendingId, "p2");
    assert.equal(request.actionId, first.action_id);
    assert.equal(request.requestedApprover, undefined);
    assert.equal(request.approvalRouteId, undefined);
    assert.equal(request.approvalDeliveryId, undefined);
    assert.equal(request.approvalRecipient, "review-queue");
    assert.equal(request.approvalChannel, "notification");
    assert.equal(request.deadlineAtMs, first.deadline_at_ms);
  } finally { resource.dispose(); }
});

test("one resource discovers approvals without an inbox, pauses while hidden and refreshes on return", async () => {
  const c = clock(); let calls = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment, load: async () => ({ pending: ++calls === 1 ? [] : [row()] }), decide: async () => ({}) });
  await tick(); assert.equal(resource.getSnapshot().requests.length, 0);
  c.advance(15_000); await tick(); assert.equal(resource.getSnapshot().requests.length, 1);
  assert.equal(calls, 2);
  c.visible(false); c.advance(60_000); await tick(); assert.equal(calls, 2);
  c.visible(true); await tick(); assert.equal(calls, 3);
  resource.dispose(); assert.equal(c.timers.size, 0);
});

test("slow polls never overlap, stale after two intervals, deadline alone does not settle", async () => {
  const c = clock(); const slow = deferred<unknown>(); let calls = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment, load: async () => ++calls === 1 ? { pending: [row()] } : slow.promise, decide: async () => ({}) });
  await tick(); c.advance(30_000); await tick();
  assert.equal(calls, 2); assert.equal(resource.getSnapshot().status, "stale");
  assert.equal(resource.getSnapshot().requests[0].status, "pending");
  c.advance(30_000); await tick(); assert.equal(calls, 2);
  slow.resolve({ pending: [] }); await tick(); assert.equal(resource.getSnapshot().status, "ready");
  resource.dispose();
});

test("read failures preserve stale data, denial clears it and suppresses future mutation", async () => {
  const c = clock(); let stage = 0, writes = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: async () => { if (stage === 1) throw new Error("network"); if (stage === 2) throw Object.assign(new Error("forbidden"), { httpStatus: 403 }); return { pending: [row()] }; },
    decide: async () => { writes++; return accepted(); },
  });
  await tick(); stage = 1; await resource.refresh(); assert.equal(resource.getSnapshot().status, "stale"); assert.equal(resource.getSnapshot().requests.length, 1);
  stage = 2; await resource.refresh(); assert.equal(resource.getSnapshot().status, "forbidden"); assert.equal(resource.getSnapshot().requests.length, 0);
  await resource.decide("p1", "approve"); assert.equal(writes, 0); resource.dispose();
});

test("scope disposal aborts inflight read and ignores late result", async () => {
  const c = clock(), slow = deferred<unknown>(); let signal!: AbortSignal;
  const resource = createPendingApprovalResource({ scopeKey: "old", environment: c.environment, load: async (s) => { signal = s; return slow.promise; }, decide: async () => ({}) });
  await tick(); resource.dispose(); slow.resolve({ pending: [row()] }); await tick();
  assert.equal(signal.aborted, true); assert.equal(resource.getSnapshot().requests.length, 0); assert.equal(c.timers.size, 0);
});

test("simultaneous panes share one decision, reconcile competing resolution and escalation", async () => {
  const c = clock(), decision = deferred<unknown>(); let reads = 0, writes = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment, load: async () => ({ pending: ++reads === 1 ? [row()] : [row("p2")] }), decide: async () => { writes++; return decision.promise; } });
  await tick(); const one = resource.decide("p1", "escalate"), two = resource.decide("p1", "approve");
  assert.equal(one, two); await tick(); assert.equal(writes, 1); assert.equal(resource.getSnapshot().decisions.p1.phase, "submitting");
  decision.resolve(accepted("p1", "escalate")); await one;
  assert.equal(resource.getSnapshot().decisions.p1.result?.next_pending_id, "p2"); assert.equal(resource.getSnapshot().requests[0].pendingId, "p2"); resource.dispose();
});

test("read-only users cannot decide and an unknown response never invents success", async () => {
  const c = clock(); let writes = 0;
  const readOnly = createPendingApprovalResource({ scopeKey: "a", environment: c.environment, readOnly: true, load: async () => ({ pending: [row()] }), decide: async () => { writes++; return accepted(); } });
  await tick(); await readOnly.decide("p1", "approve"); assert.equal(writes, 0); readOnly.dispose();
  const resource = createPendingApprovalResource({ scopeKey: "b", environment: c.environment, load: async () => ({ pending: [row()] }), decide: async () => ({ ok: true }) });
  await tick(); await resource.decide("p1", "approve"); assert.equal(resource.getSnapshot().decisions.p1.phase, "failed"); assert.equal(resource.getSnapshot().requests.length, 1); resource.dispose();
});

test("correlation requires exact owner origin and never uses actor_id", () => {
  const unscoped = normalizePendingApproval({ ...row(), actor_id: "identity:a" })!;
  assert.equal(approvalMatchesConversation(unscoped, { identity: "identity:a", conversationId: "c" }), false);
  const scoped = normalizePendingApproval({ ...row(), origin: { identity: "identity:a", conversation_id: "c", interaction_id: "i" } })!;
  assert.equal(approvalMatchesConversation(scoped, { identity: "identity:a", conversationId: "c", interactionIds: ["i"] }), true);
  assert.equal(approvalMatchesConversation(scoped, { identity: "identity:b", conversationId: "c", interactionIds: ["i"] }), false);
  assert.equal(approvalMatchesConversation(scoped, { identity: "identity:a", conversationId: "different", interactionIds: ["i"] }), false);
});

test("a poll started before a decision cannot resurrect the settled request", async () => {
  const c = clock(), slow = deferred<unknown>(); let reads = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: async () => { reads++; return reads === 1 ? { pending: [row()] } : reads === 2 ? slow.promise : { pending: [] }; }, decide: async () => accepted(),
  });
  await tick(); c.advance(15_000); await tick();
  const deciding = resource.decide("p1", "approve"); await tick();
  assert.equal(resource.getSnapshot().requests.length, 0);
  slow.resolve({ pending: [row()] }); await deciding; await tick();
  assert.equal(resource.getSnapshot().requests.length, 0); assert.equal(reads, 3);
  resource.dispose();
});

test("late read after decision permission revocation cannot restore secret pending data", async () => {
  const c = clock(), slow = deferred<unknown>(); let reads = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: async () => ++reads === 1 ? { pending: [row()] } : slow.promise,
    decide: async () => { throw Object.assign(new Error("permission revoked"), { httpStatus: 403 }); },
  });
  await tick(); c.advance(15_000); await tick();
  await resource.decide("p1", "approve");
  slow.resolve({ pending: [row()] }); await tick();
  assert.equal(resource.getSnapshot().status, "forbidden"); assert.equal(resource.getSnapshot().requests.length, 0); resource.dispose();
});

test("synchronous loader errors release the polling slot and repeated explicit invalidations coalesce", async () => {
  const c = clock(), slow = deferred<unknown>(); let calls = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: () => { calls++; if (calls === 1) throw new Error("sync failure"); if (calls === 2) return slow.promise; return Promise.resolve({ pending: [] }); }, decide: async () => ({}),
  });
  await tick(); assert.equal(resource.getSnapshot().status, "unavailable");
  const refresh = resource.refresh(); resource.refresh(); resource.refresh(); await tick(); assert.equal(calls, 2);
  slow.resolve({ pending: [row()] }); await refresh; await tick(); assert.equal(calls, 3); resource.dispose();
});

test("revocation clears decision history and a late failed read cannot downgrade forbidden", async () => {
  const c = clock(), slow = deferred<unknown>(); let reads = 0;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: async () => ++reads === 1 ? { pending: [row()] } : slow.promise,
    decide: async () => { throw Object.assign(new Error("permission revoked"), { httpStatus: 403 }); },
  });
  await tick(); c.advance(15_000); await tick(); await resource.decide("p1", "approve");
  slow.reject(new Error("network failed after revocation")); await tick();
  assert.equal(resource.getSnapshot().status, "forbidden");
  assert.deepEqual(resource.getSnapshot().decisions, {});
  assert.equal(resource.getSnapshot().requests.length, 0); resource.dispose();
});

test("decision scope, outcome and escalation successor must all match the owner contract", async () => {
  for (const response of [
    { ...accepted(), action_id: "different action" },
    { ...accepted(), outcome: "safe_draft" },
    { ...accepted("p1", "escalate"), next_pending_id: undefined },
    { ...accepted("p1", "escalate"), next_pending_id: "p1" },
  ]) {
    const c = clock();
    const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
      load: async () => ({ pending: [row()] }), decide: async () => response,
    });
    await tick(); await resource.decide("p1", response.decision as "approve" | "escalate");
    assert.equal(resource.getSnapshot().decisions.p1.phase, "failed");
    assert.equal(resource.getSnapshot().requests.length, 1); resource.dispose();
  }
});

test("a competing resolution removes the request on refresh without inventing our decision success", async () => {
  const c = clock(); let resolvedElsewhere = false;
  const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
    load: async () => ({ pending: resolvedElsewhere ? [] : [row()] }),
    decide: async () => { resolvedElsewhere = true; throw new Error("request already resolved"); },
  });
  await tick(); await resource.decide("p1", "approve");
  assert.equal(resource.getSnapshot().requests.length, 0);
  assert.equal(resource.getSnapshot().decisions.p1.phase, "failed");
  assert.equal(resource.getSnapshot().decisions.p1.result, undefined); resource.dispose();
});

for (const [name, create] of [["shared", sharedController], ["stock", stockController]] as const) {
  test(`${name}: fresh capability removal clears unreadable pending but preserves a readable read-only inbox`, async () => {
    const c = clock(); let methods = [CONSOLE_RPC_METHODS.gatingPending, CONSOLE_RPC_METHODS.gatingDecide] as string[];
    let writes = 0;
    const target = migrateConsoleWorkbenchTarget({ id: "gating", kind: "gating", title: "Approvals" })!;
    const transport = {
      loadExperience: async () => ({}), capabilities: async () => ({ methods }),
      executeCommand: async (input: { command: string }) => ({ command: input.command, accepted: true,
        result: input.command === CONSOLE_COMMAND_NAMES.listGatingPending ? { pending: [row()] } : (writes++, accepted()) }),
    } as unknown as MetaConsoleTransport;
    const controller = create({ transport });
    const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
      load: async signal => (await controller.commands.execute({ command: CONSOLE_COMMAND_NAMES.listGatingPending, target, signal })).result,
      decide: async (_id, _action, signal) => (await controller.commands.execute({ command: CONSOLE_COMMAND_NAMES.decideGating, target, signal })).result,
    });
    await tick(); assert.equal(resource.getSnapshot().requests.length, 1);
    methods = [CONSOLE_RPC_METHODS.gatingPending];
    await resource.decide("p1", "approve");
    assert.equal(writes, 0); assert.equal(resource.getSnapshot().readOnly, true);
    assert.equal(resource.getSnapshot().decisions.p1.phase, "unavailable", "a known pre-dispatch capability refusal is not an uncertain decision");
    assert.equal(resource.getSnapshot().status, "ready"); assert.equal(resource.getSnapshot().requests.length, 1);
    await resource.decide("p1", "approve"); assert.equal(writes, 0);
    methods = [];
    await resource.refresh(); assert.equal(resource.getSnapshot().status, "unsupported");
    assert.equal(resource.getSnapshot().requests.length, 0); assert.deepEqual(resource.getSnapshot().decisions, {});
    resource.dispose();
  });

  test(`${name}: lost acknowledgement remains uncertain when a retry is refused before dispatch`, async () => {
    const c = clock(); let methods = [CONSOLE_RPC_METHODS.gatingPending, CONSOLE_RPC_METHODS.gatingDecide] as string[];
    let writes = 0;
    const target = migrateConsoleWorkbenchTarget({ id: "gating", kind: "gating", title: "Approvals" })!;
    const message = "Approval decisions are unavailable with current access";
    const controller = create({ transport: {
      loadExperience: async () => ({}), capabilities: async () => ({ methods }),
      executeCommand: async (input: { command: string }) => {
        if (input.command === CONSOLE_COMMAND_NAMES.listGatingPending) {
          return { command: input.command, accepted: true, result: { pending: [row()] } };
        }
        writes++;
        // Error prose cannot establish whether this dispatched command was accepted.
        throw new Error(message);
      },
    } as unknown as MetaConsoleTransport });
    const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
      load: async signal => (await controller.commands.execute({ command: CONSOLE_COMMAND_NAMES.listGatingPending, target, signal })).result,
      decide: async (_id, _action, signal) => (await controller.commands.execute({ command: CONSOLE_COMMAND_NAMES.decideGating, target, signal })).result,
    });
    try {
      await tick(); await resource.decide("p1", "approve");
      assert.equal(writes, 1);
      assert.equal(resource.getSnapshot().readOnly, false);
      assert.deepEqual(resource.getSnapshot().decisions.p1, { phase: "failed", action: "approve", error: message });
      methods = [CONSOLE_RPC_METHODS.gatingPending];
      await resource.decide("p1", "reject");
      assert.equal(writes, 1, "the retry is refused before transport dispatch");
      assert.equal(resource.getSnapshot().readOnly, true);
      assert.deepEqual(resource.getSnapshot().decisions.p1, { phase: "failed", action: "approve", error: message }, "a later local refusal cannot settle the earlier unknown outcome");
      assert.equal(resource.getSnapshot().requests.length, 1);
    } finally { resource.dispose(); }
  });

  test(`${name}: failed capability fetch remains transient instead of inventing permission denial`, async () => {
    const c = clock(); let failing = false;
    const target = migrateConsoleWorkbenchTarget({ id: "gating", kind: "gating", title: "Approvals" })!;
    const controller = create({ transport: {
      loadExperience: async () => ({}),
      capabilities: async () => { if (failing) throw new Error("offline"); return { methods: [CONSOLE_RPC_METHODS.gatingPending] }; },
      executeCommand: async () => ({ command: CONSOLE_COMMAND_NAMES.listGatingPending, accepted: true, result: { pending: [row()] } }),
    } as unknown as MetaConsoleTransport });
    const resource = createPendingApprovalResource({ scopeKey: "a", environment: c.environment,
      load: async signal => (await controller.commands.execute({ command: CONSOLE_COMMAND_NAMES.listGatingPending, target, signal })).result,
      decide: async () => accepted(),
    });
    await tick(); failing = true; await resource.refresh();
    assert.equal(resource.getSnapshot().status, "stale"); assert.equal(resource.getSnapshot().readOnly, false);
    assert.equal(resource.getSnapshot().requests.length, 1); resource.dispose();
  });
}

test("unadvertised gating is unsupported, stops polling and cannot accept a decision", async () => {
  const c = clock(); let calls = 0, writes = 0;
  const resource = createPendingApprovalResource({ scopeKey: "no-gating", environment: c.environment,
    load: async () => { calls++; throw Object.assign(new Error("method unavailable"), { kind: "console-capability-unavailable", method: CONSOLE_RPC_METHODS.gatingPending, availableMethods: [] }); },
    decide: async () => { writes++; return accepted(); },
  });
  await tick();
  assert.equal(resource.getSnapshot().status, "unsupported");
  assert.equal(resource.getSnapshot().readOnly, true);
  assert.deepEqual(resource.getSnapshot().requests, []);
  c.advance(60_000); await tick(); await resource.refresh(); await resource.decide("p1", "approve");
  assert.equal(calls, 1); assert.equal(writes, 0);
  resource.dispose();
});
