/**
 * Mob announcement sources on the TypeScript SDK: wire names, typed results,
 * open vocabularies, typed errors and the capability seam. The transport is
 * mocked; each row checks the exact method name and params the SDK sends and
 * how it reads the gateway's answer.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";

import * as sdk from "../dist/index.js";
import type {
  AnnouncementConflictError as ConflictError,
  AnnouncementRefusedError as RefusedError,
  MobHandle,
  SourceStaleRevisionError as StaleRevisionError,
  SourceVersionConflictError as VersionConflictError,
} from "../dist/index.js";

const { Meta, MetaRuntime, RpcError, parseCapabilitiesResult, parseErrorEvent } = sdk;

// The announcement names are read from the module namespace, so a missing
// export fails only the rows that use it.
const api = sdk as unknown as Record<string, any>;
const supportsAnnouncementSources = (value: unknown): boolean => api.supportsAnnouncementSources(value);
const parseAnnouncementRpcError = (...args: unknown[]): Error | null => api.parseAnnouncementRpcError(...args);
const parseAnnouncement = (raw: unknown): any => api.parseAnnouncement(raw);
const errorClass = (name: string): Function => {
  const cls = api[name];
  assert.equal(typeof cls, "function", `${name} is exported`);
  return cls;
};

const MEMBER = { mob_id: "mob-a", identity: "lead:main" };

const ANNOUNCEMENT = {
  mob_id: "mob-a",
  sequence: 7,
  announced_by: { kind: "host" },
  source_key: "ops/1",
  topics: ["ops.alert"],
  body: "Disk full on db-2",
  correlation_id: "c-1",
  causes: [{ mob_id: "mob-a", source_key: "ops/0" }],
  digest: "sha256:ab",
  announced_at_ms: 1000,
  a_future_field: true,
};

const SUBSCRIPTION = {
  id: "sub-1",
  member: MEMBER,
  selector: { topics: ["ops.alert"], mobs: ["mob-a"] },
  state: "active",
  revision: 2,
  selector_revision: 1,
  frontier: { "mob-a": 6 },
  created_at_ms: 10,
  updated_at_ms: 20,
};

const BATCH = {
  id: "batch-1",
  member: MEMBER,
  items: [
    { subscription_id: "sub-1", subscription_revision: 2, mob_id: "mob-a", sequence: 7, source_key: "ops/1" },
  ],
  gaps: [{ mob_id: "mob-a", count: 2, reason: "expired" }],
  route: "local",
  body: "1 announcement",
  content_digest: "sha256:cd",
  state: { state: "delivered", receipt: { delivery_id: "d-1" } },
  sealed_at_ms: 30,
  updated_at_ms: 40,
  handling: { state: "handled" },
};

const PRINCIPAL = {
  kind: "connector_verified_account",
  issuer: "https://issuer.example",
  strategy_id: "oauth",
  subject: "user-1",
};

const SOURCE = {
  mob_id: "mob-a",
  source_id: "mail",
  generation: 1,
  revision: 3,
  identity: { provider: "mail", principal: PRINCIPAL, scope: { kind: "inbox", id: "INBOX" } },
  connection: { kind: "connect_setup", id: "conn-1" },
  observation: { baseline: "snapshot", deletions: "explicit" },
  topics: ["mail.new"],
  audience: [{ kind: "role", mob_id: "mob-a", role: "lead" }],
  routes: [{ kind: "direct" }, { kind: "a_future_route", lane: 2 }],
  conversation_members: [],
  start: { kind: "baseline" },
  delivery: { priority: "normal", max_per_batch: 20, on_unhandled: "redeliver" },
  health_policy: {
    escalate_after_failures: 3,
    escalate_after_ms: 3600000,
    stale_after_ms: 7200000,
    quiet_after_ms: null,
    empty_after_ms: null,
  },
  runner: { host_runnable: "mail_poll", every_seconds: 300 },
  state: "active",
  collection: "live",
  cursor: { version: 4, token: "opaque-token", advanced_at_ms: 50 },
  streams: { live: 1, onboarding: 0 },
  backfilled: 0,
  health: {
    status: "ok",
    last_success_at_ms: 50,
    last_new_item_at_ms: 40,
    last_observed: { kind: "delta" },
    last_observed_at_ms: 50,
    empty_snapshot_since_ms: null,
    consecutive_failures: 0,
    totals: { announced: 5, replayed: 1, conflicts: 0, own_writes: 0, refusals: 0, gaps: 0 },
    last_failure: null,
    last_conflict: null,
  },
  created_at_ms: 1,
  updated_at_ms: 50,
};

const ADVANCE = {
  source: SOURCE,
  replayed: false,
  items: [
    { item_id: "m1", version: "v1", source_key: "src:mail/m1", outcome: "announced", sequence: 8, captured: 2 },
    { item_id: "m2", version: "v2", source_key: "src:mail/m2", outcome: "a_future_outcome" },
  ],
  streams: [{ stream: "INBOX", phase: "live" }],
};

const AUDIT_ENTRY = {
  id: 1,
  at_ms: 9,
  action: "capture_withheld",
  mob_id: "mob-a",
  member: { mob_id: "mob-a", identity: "research:1" },
  source_id: "mail",
  generation: 1,
  reason: "no_direct_route",
  count: 3,
  first_at_ms: 5,
  last_at_ms: 9,
};

const RESPONSES: Record<string, unknown> = {
  "meta/mob_announcements/announce": { announcement: ANNOUNCEMENT, replayed: false, captured: 2 },
  "meta/mob_announcements/get": { announcement: ANNOUNCEMENT },
  "meta/mob_announcements/subscriptions/put": { subscription: SUBSCRIPTION },
  "meta/mob_announcements/subscriptions/pause": { subscription: SUBSCRIPTION, unresolved_batches: [] },
  "meta/mob_announcements/subscriptions/resume": { subscription: SUBSCRIPTION, unresolved_batches: [] },
  "meta/mob_announcements/subscriptions/cancel": { subscription: SUBSCRIPTION, unresolved_batches: ["batch-1"] },
  "meta/mob_announcements/subscriptions/get": { subscription: SUBSCRIPTION, marker: null },
  "meta/mob_announcements/subscriptions/list": { subscriptions: [SUBSCRIPTION] },
  "meta/mob_announcements/deliveries/list": { deliveries: [BATCH] },
  "meta/mob_announcements/deliveries/observe": { batch: BATCH, native: { status: "not_committed" } },
  "meta/mob_announcements/deliveries/reconcile": { batch: BATCH, native: { status: "not_committed" } },
  "meta/mob_announcements/sources/put": { source: SOURCE },
  "meta/mob_announcements/sources/pause": { source: SOURCE },
  "meta/mob_announcements/sources/resume": { source: SOURCE },
  "meta/mob_announcements/sources/end": { source: SOURCE },
  "meta/mob_announcements/sources/get": {
    source: SOURCE,
    streams: [{ stream: "INBOX", phase: "live", since_ms: 5, backfilled: 0 }],
  },
  "meta/mob_announcements/sources/list": { sources: [SOURCE] },
  "meta/mob_announcements/sources/advance": ADVANCE,
  "meta/mob_announcements/sources/record_failure": { source: SOURCE },
  "meta/mob_announcements/audit/list": { entries: [AUDIT_ENTRY] },
};

interface RpcCall {
  method: string;
  params: Record<string, unknown> | undefined;
}

function newRuntime(): InstanceType<typeof MetaRuntime> {
  const rt = new MetaRuntime({
    mobConfigPath: null,
    sessionBuilder: null,
    errorCallback: null,
    eventLog: null,
    consoleConfigPath: null,
    consoleRequireAppAuth: null,
    consoleReadOnly: null,
    consoleFetchTimeoutMs: null,
    gatingConfigPath: null,
    routingConfigPath: null,
    workgraphEnabled: null,
    memoryConfig: null,
    agentMemoryConfig: null,
    authConfig: null,
    implicitDelegateIdleRetireSecs: undefined,
    maxSessions: null,
    gatewayTimeoutMs: null,
    gatewayBin: null,
    modules: [],
    persistentState: null,
    rosterProvider: null,
    agentCustomizer: null,
    topologyProvider: null,
  } as any);
  (rt as any)._running = true;
  return rt;
}

/** A handle whose RPC layer records each call and answers from `answer`. */
function mockHandle(
  answer: (method: string) => unknown = (method) => RESPONSES[method] ?? {},
): { handle: MobHandle; calls: RpcCall[] } {
  const rt = newRuntime();
  const calls: RpcCall[] = [];
  (rt as any)._rpc = async (method: string, params?: Record<string, unknown>) => {
    calls.push({ method, params });
    return answer(method);
  };
  return { handle: rt.mobHandle(), calls };
}

/** A handle whose transport answers every request with one JSON-RPC error. */
function failingHandle(error: Record<string, unknown>): MobHandle {
  const rt = newRuntime();
  (rt as any)._transport = {
    sendAsync: async (request: Record<string, unknown>) => ({ jsonrpc: "2.0", id: request.id, error }),
  };
  return rt.mobHandle();
}

const SERVED: [string, (h: MobHandle) => Promise<unknown>, string, Record<string, unknown>][] = [
  ["announce", (h) => h.announce("ops/1", ["ops.alert"], "Disk full", { correlationId: "c-1" }),
    "meta/mob_announcements/announce",
    { source_key: "ops/1", topics: ["ops.alert"], body: "Disk full", correlation_id: "c-1" }],
  ["announcementGet", (h) => h.announcementGet("mob-a", "ops/1"),
    "meta/mob_announcements/get", { mob_id: "mob-a", source_key: "ops/1" }],
  ["announcementSubscriptionPut",
    (h) => h.announcementSubscriptionPut("op-1", "sub-1", MEMBER, ["ops.alert"], { mobs: ["mob-a"] }),
    "meta/mob_announcements/subscriptions/put",
    { operation_id: "op-1", subscription_id: "sub-1", member: MEMBER, topics: ["ops.alert"], mobs: ["mob-a"] }],
  ["announcementSubscriptionPause", (h) => h.announcementSubscriptionPause("op-2", "sub-1", 2),
    "meta/mob_announcements/subscriptions/pause",
    { operation_id: "op-2", subscription_id: "sub-1", expected_revision: 2 }],
  ["announcementSubscriptionResume", (h) => h.announcementSubscriptionResume("op-3", "sub-1", 3),
    "meta/mob_announcements/subscriptions/resume",
    { operation_id: "op-3", subscription_id: "sub-1", expected_revision: 3 }],
  ["announcementSubscriptionCancel", (h) => h.announcementSubscriptionCancel("op-4", "sub-1", 4),
    "meta/mob_announcements/subscriptions/cancel",
    { operation_id: "op-4", subscription_id: "sub-1", expected_revision: 4 }],
  ["announcementSubscriptionGet", (h) => h.announcementSubscriptionGet("sub-1"),
    "meta/mob_announcements/subscriptions/get", { subscription_id: "sub-1" }],
  ["announcementSubscriptionList", (h) => h.announcementSubscriptionList({ member: MEMBER, limit: 5 }),
    "meta/mob_announcements/subscriptions/list", { member: MEMBER, limit: 5 }],
  ["announcementDeliveryList", (h) => h.announcementDeliveryList({ subscriptionId: "sub-1" }),
    "meta/mob_announcements/deliveries/list", { subscription_id: "sub-1" }],
  ["announcementDeliveryObserve", (h) => h.announcementDeliveryObserve("batch-1"),
    "meta/mob_announcements/deliveries/observe", { batch_id: "batch-1" }],
  ["announcementDeliveryReconcile", (h) => h.announcementDeliveryReconcile("batch-1"),
    "meta/mob_announcements/deliveries/reconcile", { batch_id: "batch-1" }],
  ["announcementSourcePut",
    (h) => h.announcementSourcePut("op-5", "mail", SOURCE.identity, SOURCE.observation, ["mail.new"],
      SOURCE.audience, { runner: { host_runnable: "mail_poll", every_seconds: 300 } }),
    "meta/mob_announcements/sources/put",
    { operation_id: "op-5", source_id: "mail", identity: SOURCE.identity, observation: SOURCE.observation,
      topics: ["mail.new"], audience: SOURCE.audience,
      runner: { host_runnable: "mail_poll", every_seconds: 300 } }],
  ["announcementSourcePause", (h) => h.announcementSourcePause("op-6", "mail", 3),
    "meta/mob_announcements/sources/pause", { operation_id: "op-6", source_id: "mail", expected_revision: 3 }],
  ["announcementSourceResume", (h) => h.announcementSourceResume("op-7", "mail", 4, { rebaseline: true }),
    "meta/mob_announcements/sources/resume",
    { operation_id: "op-7", source_id: "mail", expected_revision: 4, rebaseline: true }],
  ["announcementSourceEnd", (h) => h.announcementSourceEnd("op-8", "mail", 5),
    "meta/mob_announcements/sources/end", { operation_id: "op-8", source_id: "mail", expected_revision: 5 }],
  ["announcementSourceGet", (h) => h.announcementSourceGet("mail", { includeStreams: true }),
    "meta/mob_announcements/sources/get", { source_id: "mail", include_streams: true }],
  ["announcementSourceList", (h) => h.announcementSourceList({ state: "active", limit: 10 }),
    "meta/mob_announcements/sources/list", { state: "active", limit: 10 }],
  ["announcementSourceAdvance",
    (h) => h.announcementSourceAdvance("op-9", "mail", 3, 4, PRINCIPAL, "cursor-5",
      { items: [{ item_id: "m1" }], observed: { kind: "delta" } }),
    "meta/mob_announcements/sources/advance",
    { operation_id: "op-9", source_id: "mail", expected_revision: 3, expected_version: 4,
      principal: PRINCIPAL, next_cursor: "cursor-5", items: [{ item_id: "m1" }], observed: { kind: "delta" } }],
  ["announcementSourceRecordFailure",
    (h) => h.announcementSourceRecordFailure("op-10", "mail", 3, 4, PRINCIPAL,
      { class: "rate_limited", retry_after_ms: 60000 }),
    "meta/mob_announcements/sources/record_failure",
    { operation_id: "op-10", source_id: "mail", expected_revision: 3, expected_version: 4,
      principal: PRINCIPAL, failure: { class: "rate_limited", retry_after_ms: 60000 } }],
  ["announcementAuditList", (h) => h.announcementAuditList({ sourceId: "mail", after: 4 }),
    "meta/mob_announcements/audit/list", { source_id: "mail", after: 4 }],
];

describe("MobHandle mob announcement wire", () => {
  for (const [name, call, method, params] of SERVED) {
    it(`${name} sends ${method} with its params`, async () => {
      const { handle, calls } = mockHandle();
      await call(handle);
      assert.deepEqual(calls, [{ method, params }]);
    });
  }
});

describe("MobHandle mob announcement results", () => {
  it("announce reads the announcement and counts captures", async () => {
    const { handle } = mockHandle();
    const result = await handle.announce("ops/1", ["ops.alert"], "Disk full");
    assert.equal(result.replayed, false);
    assert.equal(result.captured, 2);
    assert.equal(result.announcement.sequence, 7);
    assert.equal(result.announcement.sourceKey, "ops/1");
    assert.equal(result.announcement.announcedBy.kind, "host");
    assert.deepEqual(result.announcement.topics, ["ops.alert"]);
    assert.equal(result.announcement.causes[0].sourceKey, "ops/0");
  });

  it("announcementGet answers null for an unknown key", async () => {
    const { handle } = mockHandle(() => ({ announcement: null }));
    assert.equal(await handle.announcementGet("mob-a", "ops/9"), null);
  });

  it("an unknown announcer kind keeps its raw object", () => {
    const parsed = parseAnnouncement({ ...ANNOUNCEMENT, announced_by: { kind: "a_future_announcer", lane: 3 } });
    assert.equal(parsed.announcedBy.kind, "a_future_announcer");
    assert.deepEqual(parsed.announcedBy.raw, { kind: "a_future_announcer", lane: 3 });
    const source = parseAnnouncement({ ...ANNOUNCEMENT, announced_by: { kind: "source", source_id: "mail" } });
    assert.equal(source.announcedBy.sourceId, "mail");
  });

  it("subscription records keep an open state and the frontier", async () => {
    const { handle } = mockHandle(() => ({ subscriptions: [{ ...SUBSCRIPTION, state: "a_future_state" }] }));
    const [subscription] = await handle.announcementSubscriptionList();
    assert.equal(subscription.state, "a_future_state");
    assert.deepEqual(subscription.frontier, { "mob-a": 6 });
    assert.equal(subscription.member.identity, "lead:main");
    assert.deepEqual(subscription.topics, ["ops.alert"]);
    assert.deepEqual(subscription.mobs, ["mob-a"]);
  });

  it("a subscription change reads its unresolved batches", async () => {
    const { handle } = mockHandle();
    const change = await handle.announcementSubscriptionCancel("op-4", "sub-1", 4);
    assert.equal(change.subscription?.id, "sub-1");
    assert.deepEqual(change.unresolvedBatches, ["batch-1"]);
  });

  it("subscription get reads the marker, the backlog and an absent subscription", async () => {
    const marker = { cause: { kind: "peer_composing" }, evidence: "observed", since_ms: 5 };
    let { handle } = mockHandle(() => ({
      subscription: SUBSCRIPTION, marker, backlog: { eligible_count: 3, outstanding: true },
    }));
    let view = await handle.announcementSubscriptionGet("sub-1");
    assert.equal(view.subscription?.id, "sub-1");
    assert.equal(view.marker?.causeKind, "peer_composing");
    assert.equal(view.marker?.sinceMs, 5);
    assert.deepEqual(view.backlog, { eligible_count: 3, outstanding: true });
    ({ handle } = mockHandle(() => ({ subscription: null, marker: null })));
    view = await handle.announcementSubscriptionGet("sub-9");
    assert.equal(view.subscription, null);
    assert.equal(view.marker, null);
    assert.equal(view.backlog, null);
  });

  it("deliveries read the batch state, handling and gaps", async () => {
    const { handle } = mockHandle();
    const [batch] = await handle.announcementDeliveryList();
    assert.equal(batch.id, "batch-1");
    assert.equal(batch.state, "delivered");
    assert.deepEqual(batch.stateDetail, { state: "delivered", receipt: { delivery_id: "d-1" } });
    assert.equal(batch.handling?.state, "handled");
    assert.equal(batch.route, "local");
    assert.equal(batch.items[0].sequence, 7);
    assert.equal(batch.gaps[0].reason, "expired");
  });

  it("delivery observe reads the observation", async () => {
    const { handle } = mockHandle();
    const observation = await handle.announcementDeliveryObserve("batch-1");
    assert.equal(observation.batch.id, "batch-1");
    assert.deepEqual(observation.native, { status: "not_committed" });
    assert.equal(observation.nativeStatus, "not_committed");
    assert.equal(observation.marker, null);
  });

  it("a source record reads typed fields and keeps its vocabulary open", async () => {
    const { handle } = mockHandle(() => ({
      sources: [{
        ...SOURCE, state: "a_future_state", collection: "a_future_phase",
        health: { ...SOURCE.health, status: "a_future_status" },
      }],
    }));
    const [source] = await handle.announcementSourceList();
    assert.equal(source.sourceId, "mail");
    assert.equal(source.generation, 1);
    assert.equal(source.revision, 3);
    assert.equal(source.state, "a_future_state");
    assert.equal(source.collection, "a_future_phase");
    assert.equal(source.health.status, "a_future_status");
    assert.equal(source.health.totals.announced, 5);
    assert.equal(source.cursor.version, 4);
    assert.equal(source.cursor.token, "opaque-token");
    assert.equal(source.runner?.hostRunnable, "mail_poll");
    assert.equal(source.runner?.everySeconds, 300);
    assert.equal((source.identity.principal as Record<string, unknown>).kind, "connector_verified_account");
    assert.deepEqual(source.routes.map((route) => route.kind), ["direct", "a_future_route"]);
    assert.deepEqual(source.routes[1].raw, { kind: "a_future_route", lane: 2 });
    assert.deepEqual(source.topics, ["mail.new"]);
    assert.deepEqual(source.streams, { live: 1, onboarding: 0 });
  });

  it("source put reads affected subscriptions and delivery readiness", async () => {
    let { handle } = mockHandle(() => ({
      source: SOURCE, affected_subscriptions: 4, delivery_readiness: { kind: "ready" },
    }));
    let put = await handle.announcementSourcePut("op-5", "mail", SOURCE.identity, SOURCE.observation,
      ["mail.new"], SOURCE.audience);
    assert.equal(put.source.sourceId, "mail");
    assert.equal(put.affectedSubscriptions, 4);
    assert.deepEqual(put.deliveryReadiness, { kind: "ready" });
    ({ handle } = mockHandle());
    put = await handle.announcementSourcePut("op-5", "mail", SOURCE.identity, SOURCE.observation,
      ["mail.new"], SOURCE.audience);
    assert.equal(put.affectedSubscriptions, null);
    assert.equal(put.deliveryReadiness, null);
  });

  it("source get reads the streams when asked", async () => {
    const { handle } = mockHandle();
    const view = await handle.announcementSourceGet("mail", { includeStreams: true });
    assert.equal(view.source?.sourceId, "mail");
    assert.equal(view.streams?.[0].phase, "live");
    assert.equal(view.streams?.[0].sinceMs, 5);
  });

  it("source get answers null for an unknown source", async () => {
    const { handle } = mockHandle(() => ({ source: null }));
    const view = await handle.announcementSourceGet("no-such-source");
    assert.equal(view.source, null);
    assert.equal(view.streams, null);
  });

  it("source advance reads item outcomes and streams", async () => {
    const { handle } = mockHandle();
    const advance = await handle.announcementSourceAdvance("op-9", "mail", 3, 4, PRINCIPAL, "cursor-5");
    assert.equal(advance.replayed, false);
    const [first, second] = advance.items;
    assert.equal(first.outcome, "announced");
    assert.equal(first.sequence, 8);
    assert.equal(first.captured, 2);
    assert.equal(second.outcome, "a_future_outcome");
    assert.equal(second.sequence, null);
    assert.equal(advance.streams[0].stream, "INBOX");
    assert.equal(advance.streams[0].phase, "live");
  });

  it("audit list reads counted entries", async () => {
    const { handle } = mockHandle(() => ({
      entries: [AUDIT_ENTRY, { ...AUDIT_ENTRY, id: 2, action: "a_future_action" }],
    }));
    const [first, second] = await handle.announcementAuditList();
    assert.equal(first.id, 1);
    assert.equal(first.action, "capture_withheld");
    assert.equal(first.reason, "no_direct_route");
    assert.equal(first.count, 3);
    assert.equal(first.member.identity, "research:1");
    assert.equal(second.action, "a_future_action");
  });

  it("an audit entry for a plain announcement names no source", async () => {
    const plain: Record<string, unknown> = { ...AUDIT_ENTRY };
    delete plain.source_id;
    delete plain.generation;
    const { handle } = mockHandle(() => ({
      entries: [{ ...plain, action: "redelivered", reason: "unhandled" }, AUDIT_ENTRY],
    }));
    const [first, second] = await handle.announcementAuditList();
    assert.deepEqual(
      [first.action, first.reason, first.sourceId, first.generation],
      ["redelivered", "unhandled", null, null],
    );
    assert.deepEqual([second.sourceId, second.generation], ["mail", 1]);
  });
});

describe("mob announcement errors", () => {
  const error = (code: number, kind: string, data: Record<string, unknown> = {}) =>
    parseAnnouncementRpcError(code, "refused", "r-1", "meta/mob_announcements/sources/put", { kind, ...data });

  const cases: [number, string, string][] = [
    [-32044, "mob_announcements_unavailable", "MobAnnouncementsUnavailableError"],
    [-32045, "operation_reused", "AnnouncementConflictError"],
    [-32046, "gap", "AnnouncementFrontierError"],
    [-32047, "topic_owned", "AnnouncementRefusedError"],
    [-32048, "unknown_source", "AnnouncementNotFoundError"],
  ];
  for (const [code, kind, name] of cases) {
    it(`${code} ${kind} maps to ${name}`, () => {
      const cls = errorClass(name);
      const reified = error(code, kind, { topic: "mail.new", source_id: "mail" }) as InstanceType<typeof RpcError> | null;
      assert.ok(reified instanceof cls);
      assert.ok(reified instanceof RpcError);
      assert.equal(reified?.constructor, cls);
      assert.equal(reified?.code, code);
      assert.equal((reified as ConflictError).kind, kind);
    });
  }

  it("source conflicts carry the current position", () => {
    const conflictBase = errorClass("AnnouncementConflictError");
    const cursor = { version: 9, token: "t", advanced_at_ms: 1 };
    const stale = error(-32045, "stale_revision", {
      current_revision: 5, current_version: 9, generation: 2, cursor,
    }) as StaleRevisionError;
    assert.equal(stale.constructor, errorClass("SourceStaleRevisionError"));
    assert.ok(stale instanceof conflictBase);
    assert.equal(stale.currentRevision, 5);
    assert.equal(stale.currentVersion, 9);
    assert.equal(stale.generation, 2);
    assert.deepEqual(stale.cursor, cursor);
    const conflict = error(-32045, "source_version_conflict", {
      current_version: 9, cursor, state: "active", revision: 5,
    }) as VersionConflictError;
    assert.equal(conflict.constructor, errorClass("SourceVersionConflictError"));
    assert.ok(conflict instanceof conflictBase);
    assert.equal(conflict.currentVersion, 9);
    assert.equal(conflict.revision, 5);
    assert.equal(conflict.state, "active");
    const plain = error(-32045, "stale_revision", { current_revision: 3 });
    assert.equal(plain?.constructor, conflictBase);
  });

  it("an unknown refusal kind keeps the family base with its kind", () => {
    const reified = error(-32047, "a_future_refusal", { detail: 1 }) as RefusedError;
    assert.equal(reified.constructor, errorClass("AnnouncementRefusedError"));
    assert.equal(reified.kind, "a_future_refusal");
  });

  it("an unrelated error code is not an announcement error", () => {
    assert.equal(error(-32042, "stale_revision"), null);
  });

  it("a gateway conflict reaches the caller as its typed class", async () => {
    const handle = failingHandle({
      code: -32045,
      message: "stale revision",
      data: { kind: "stale_revision", current_revision: 5, current_version: 9, generation: 2, cursor: null },
    });
    await assert.rejects(
      handle.announcementSourceAdvance("op-9", "mail", 3, 4, PRINCIPAL, "cursor-5"),
      (failure: unknown) => {
        assert.equal((failure as Error).constructor, errorClass("SourceStaleRevisionError"));
        assert.equal((failure as StaleRevisionError).currentRevision, 5);
        assert.equal((failure as StaleRevisionError).method, "meta/mob_announcements/sources/advance");
        return true;
      },
    );
  });

  const newInSources: [string, (h: MobHandle) => Promise<unknown>][] = [
    ["sources/get", (h) => h.announcementSourceGet("mail")],
    ["audit/list", (h) => h.announcementAuditList()],
  ];
  for (const [name, call] of newInSources) {
    it(`${name} on an older Meta says sources are unsupported`, async () => {
      const handle = failingHandle({ code: -32601, message: "Method not found" });
      await assert.rejects(call(handle), (failure: unknown) => {
        assert.ok(failure instanceof RpcError);
        assert.equal(failure.code, -32601);
        assert.match(failure.message, /Meta does not support sources/);
        return true;
      });
    });
  }

  it("a subscription method on an older Meta keeps the plain error", async () => {
    const handle = failingHandle({ code: -32601, message: "Method not found" });
    await assert.rejects(handle.announcementSubscriptionGet("sub-1"), (failure: unknown) => {
      assert.ok(failure instanceof RpcError);
      assert.equal(failure.code, -32601);
      assert.doesNotMatch(failure.message, /sources/);
      return true;
    });
  });
});

describe("mob announcement exports", () => {
  it("exports the error codes the gateway answers with", () => {
    const codes = [
      "MOB_ANNOUNCEMENTS_UNAVAILABLE_CODE",
      "ANNOUNCEMENT_CONFLICT_CODE",
      "ANNOUNCEMENT_FRONTIER_CODE",
      "ANNOUNCEMENT_REFUSED_CODE",
      "ANNOUNCEMENT_NOT_FOUND_CODE",
    ].map((name) => api[name]);
    assert.deepEqual(codes, [-32044, -32045, -32046, -32047, -32048]);
  });

  for (const name of [
    "parseAnnouncement",
    "parseAnnouncementSubscription",
    "parseAnnouncementBatch",
    "parseAnnouncementSource",
    "parseSourceAdvanceResult",
    "parseAnnouncementAuditEntry",
    "parseAnnouncementRpcError",
    "supportsAnnouncementSources",
    "MobAnnouncementsUnavailableError",
    "AnnouncementConflictError",
    "SourceVersionConflictError",
    "SourceStaleRevisionError",
    "AnnouncementRefusedError",
    "AnnouncementNotFoundError",
    "AnnouncementFrontierError",
  ]) {
    it(`exports ${name}`, () => {
      assert.equal(typeof api[name], "function");
    });
  }
});

describe("mob announcement capability seam", () => {
  it("detects sources and reads the announcements object", () => {
    const data = {
      contract_version: "0.5.0",
      methods: ["meta/mob_announcements/announce"],
      loaded_modules: [],
      feature_capabilities: ["mob_announcements.sources.v1"],
      mob_announcements: {
        sources: "v1",
        local_delivery: { kind: "refused", cause: "governed_runtime" },
        governed_admission: false,
      },
    };
    const capabilities = parseCapabilitiesResult(data);
    assert.equal(supportsAnnouncementSources(capabilities), true);
    assert.equal(supportsAnnouncementSources(["mob_announcements.sources.v1"]), true);
    assert.equal(capabilities.mobAnnouncements?.sources, "v1");
    assert.equal(capabilities.mobAnnouncements?.governedAdmission, false);
    assert.deepEqual(capabilities.mobAnnouncements?.localDelivery, { kind: "refused", cause: "governed_runtime" });
    assert.equal(capabilities.mobAnnouncements?.localDeliveryKind, "refused");
    assert.equal(supportsAnnouncementSources(parseCapabilitiesResult({ ...data, feature_capabilities: [] })), false);
    assert.equal(parseCapabilitiesResult({ ...data, mob_announcements: null }).mobAnnouncements, null);
    const { mob_announcements: _omitted, ...absent } = data;
    assert.equal(parseCapabilitiesResult(absent).mobAnnouncements, null);
  });

  it("an escalation error event passes through before the SDK names it", () => {
    const event = parseErrorEvent({
      category: "mob_announcement_escalated", escalation_id: "e-1", cause: "source_failing",
    });
    assert.equal(event.category, "mob_announcement_escalated");
    assert.deepEqual(event.context, { escalation_id: "e-1", cause: "source_failing" });
  });
});

describe("mob announcements builder option", () => {
  const runtimeOptions = (builder: unknown): Record<string, unknown> => {
    const config = (builder as { _config: unknown })._config;
    const rt = new MetaRuntime(config as ConstructorParameters<typeof MetaRuntime>[0]);
    return (rt as any)._buildInitParams().runtime_options;
  };

  it("leaves runtime_options.mob_announcements unset by default", () => {
    assert.equal("mob_announcements" in runtimeOptions(Meta.builder()), false);
  });

  const cases: [string, (builder: any) => unknown, Record<string, unknown>][] = [
    ["enabled", (b) => b.mobAnnouncements(), { enabled: true }],
    ["disabled", (b) => b.mobAnnouncements(false), { enabled: false }],
    [
      "object",
      (b) => b.mobAnnouncements({ enabled: true, retention_ms: 3600000, batch: { max_count: 10 } }),
      { enabled: true, retention_ms: 3600000, batch: { max_count: 10 } },
    ],
  ];
  for (const [name, call, sent] of cases) {
    it(`sets runtime_options.mob_announcements (${name})`, () => {
      assert.deepEqual(runtimeOptions(call(Meta.builder())).mob_announcements, sent);
    });
  }
});

// -- A real rpc_gateway ------------------------------------------------------
//
// The rows above pin the SDK's reading of the wire. This one runs the SDK
// against the gateway itself, so the two cannot drift apart unnoticed. It runs
// when META_RPC_GATEWAY_BIN names a built rpc_gateway and makes no model call:
// the member is turn driven and nothing is delivered to it.

describe("mob announcement sources on a real gateway", () => {
  const gatewayBin = process.env.META_RPC_GATEWAY_BIN;
  const mobId = "announcement-sources-round-trip-ts";

  it("announcement_sources_round_trip", { skip: !gatewayBin && "META_RPC_GATEWAY_BIN is not set" }, async () => {
    const { mkdtempSync, writeFileSync, rmSync } = await import("node:fs");
    const { tmpdir } = await import("node:os");
    const { join } = await import("node:path");
    const dir = mkdtempSync(join(tmpdir(), "meta-announcements-"));
    const mobToml = join(dir, "mob.toml");
    writeFileSync(
      mobToml,
      `[mob]\nid = "${mobId}"\n\n[profiles.reader]\nmodel = "gpt-5.5"\nexternal_addressable = true\n\n` +
        `[profiles.reader.tools]\ncomms = true\n`,
    );
    const roster = {
      async roster() {
        return [
          {
            identity: "reader:1",
            profile: "reader",
            addressability: "addressable",
            displayName: null,
            labels: {},
            context: null,
            additionalInstructions: [],
            runtimeModeOverride: "turn_driven",
          },
        ];
      },
    };
    const runtime = await Meta.builder()
      .gateway(gatewayBin as string)
      .mob(mobToml)
      .persistentState(join(dir, "state"))
      .rosterProvider(roster as any)
      .mobAnnouncements()
      .build();
    try {
      const handle = runtime.mobHandle();
      const capabilities = await handle.capabilities();
      assert.ok(capabilities.methods.includes("meta/mob_announcements/sources/advance"));

      const first = await handle.announce("ops/1", ["ops.alert"], "Disk full on db-2", { correlationId: "c-1" });
      assert.equal(first.replayed, false);
      assert.equal(first.announcement.announcedBy.kind, "host");
      const again = await handle.announce("ops/1", ["ops.alert"], "Disk full on db-2", { correlationId: "c-1" });
      assert.equal(again.replayed, true);
      assert.equal(again.announcement.sequence, first.announcement.sequence);
      assert.equal((await handle.announcementGet(mobId, "ops/1"))?.correlationId, "c-1");
      assert.equal(await handle.announcementGet(mobId, "ops/9"), null);

      const member = { mob_id: mobId, identity: "reader:1" };
      const principal = { kind: "host_attested", attester: "round-trip-host", account: "reader@example.test" };
      const identity = { provider: "example.mail", principal, scope: { kind: "inbox", id: "INBOX" } };
      const put = await handle.announcementSourcePut(
        "op-source",
        "mail",
        identity,
        { baseline: "delta", deletions: "explicit" },
        ["mail.new"],
        [{ kind: "member", member }],
        { connection: { kind: "host", id: "conn-1" } },
      );
      const source = put.source;
      assert.deepEqual(
        [source.sourceId, source.generation, source.state, source.collection, source.cursor.version],
        ["mail", 1, "active", "baselining", 0],
      );
      assert.deepEqual(source.routes.map((route) => route.kind), ["direct"]);

      const subscription = await handle.announcementSubscriptionPut("op-sub", "sub-1", member, ["mail.new"]);
      assert.equal(subscription.state, "active");
      assert.deepEqual(subscription.mobs, [mobId]);

      const baseline = await handle.announcementSourceAdvance("op-step-0", "mail", source.revision, 0, principal, "cursor-1", {
        coverage: "complete",
        observed: { kind: "delta" },
      });
      assert.equal(baseline.source.collection, "live");
      assert.equal(baseline.source.cursor.version, 1);

      const item = { item_id: "m1", version: "v1", change: "upsert", topics: ["mail.new"], body: "A new message" };
      const step = await handle.announcementSourceAdvance(
        "op-step-1",
        "mail",
        baseline.source.revision,
        1,
        principal,
        "cursor-2",
        { items: [item] },
      );
      const [outcome] = step.items;
      assert.equal(outcome.outcome, "announced");
      assert.equal(outcome.captured, 1);
      assert.ok(outcome.sourceKey.startsWith("source:mail:"));
      const announced = await handle.announcementGet(mobId, outcome.sourceKey);
      assert.equal(announced?.announcedBy.kind, "source");
      assert.equal(announced?.announcedBy.sourceId, "mail");

      await assert.rejects(
        handle.announcementSourceAdvance("op-step-late", "mail", step.source.revision, 1, principal, "cursor-x"),
        (failure: unknown) => {
          assert.equal((failure as Error).constructor, errorClass("SourceVersionConflictError"));
          assert.equal((failure as VersionConflictError).currentVersion, 2);
          return true;
        },
      );
      await assert.rejects(
        handle.announcementSourceAdvance("op-step-stale", "mail", step.source.revision + 7, 2, principal, "cursor-x"),
        (failure: unknown) => {
          assert.equal((failure as Error).constructor, errorClass("SourceStaleRevisionError"));
          assert.equal((failure as StaleRevisionError).currentRevision, step.source.revision);
          return true;
        },
      );

      const failed = await handle.announcementSourceRecordFailure("op-failure", "mail", step.source.revision, 2, principal, {
        class: "rate_limited",
        retry_after_ms: 60000,
      });
      assert.equal(failed.health.consecutiveFailures, 1);

      const got = await handle.announcementSourceGet("mail", { includeStreams: true });
      assert.equal(got.source?.cursor.token, "cursor-2");
      assert.deepEqual(got.streams, []);
      const [listed] = await handle.announcementSourceList();
      assert.equal(listed.cursor.token, null);
      assert.deepEqual(Object.keys(listed.identity.principal).sort(), ["digest", "kind"]);
      assert.deepEqual(await handle.announcementAuditList({ sourceId: "mail" }), []);

      const paused = await handle.announcementSourcePause("op-pause", "mail", got.source?.revision ?? 0);
      assert.equal(paused.state, "paused");
      await assert.rejects(
        handle.announcementSourceAdvance("op-step-paused", "mail", paused.revision, 2, principal, "cursor-3"),
        (failure: unknown) => {
          assert.equal((failure as RefusedError).kind, "source_paused");
          return true;
        },
      );
      const ended = await handle.announcementSourceEnd("op-end", "mail", paused.revision);
      assert.equal(ended.state, "ended");
      assert.equal((await handle.announcementSourceGet("no-such-source")).source, null);
      await assert.rejects(handle.announcementSourcePause("op-pause-missing", "no-such-source", 1), (failure: unknown) => {
        assert.equal((failure as Error).constructor, errorClass("AnnouncementNotFoundError"));
        assert.equal((failure as RefusedError).kind, "unknown_source");
        return true;
      });
      for (const heldRemedy of [
        () => (handle as any).announcementDeliveryRedeliver("op-redeliver-missing", "no-such-batch"),
        () => (handle as any).announcementDeliveryDismiss("op-dismiss-missing", "no-such-batch"),
      ]) {
        await assert.rejects(heldRemedy(), (failure: unknown) => {
          assert.equal((failure as Error).constructor, errorClass("AnnouncementNotFoundError"));
          assert.equal((failure as RefusedError).kind, "unknown_delivery");
          return true;
        });
      }

      const h = handle as any;
      const nothingDeclared = await h.announcementDeclarationsGet();
      assert.deepEqual([nothingDeclared.revision, nothingDeclared.records], [0, []]);
      await assert.rejects(h.announcementDeclarationsApply(), (failure: unknown) => {
        assert.equal((failure as any).code, -32602);
        return true;
      });
      const declared = await h.announcementDeclarationsApply({
        revision: 1,
        subscriptions: [{ subscription_id: "declared-1", member, topics: ["ops.alert"] }],
      });
      assert.equal(declared.revision, 1);
      assert.ok(declared.digest.startsWith("sha256:"));
      assert.deepEqual(
        declared.records.map((record: any) => [record.kind, record.id, record.status]),
        [["subscription", "declared-1", "applied"]],
      );
      assert.equal((await h.announcementDeclarationsGet()).digest, declared.digest);
      await assert.rejects(h.announcementDeclarationsApply({ revision: 0 }), (failure: unknown) => {
        assert.equal((failure as RefusedError).kind, "declaration_rollback");
        assert.equal((failure as any).data.applied, 1);
        return true;
      });

      const readers = { mob_id: mobId, role: "reader" };
      const template = await h.announcementRoleTemplatePut("op-template", "readers", readers, ["ops.alert"]);
      assert.deepEqual([template.id, template.state, template.members.role], ["readers", "active", "reader"]);
      assert.deepEqual([template.topics, template.mobs], [["ops.alert"], [mobId]]);
      const viewed = await handle.announcementSubscriptionGet("readers");
      assert.equal(viewed.subscription, null);
      assert.equal((viewed as any).template.revision, template.revision);
      await assert.rejects(
        handle.announcementSubscriptionPut("op-taken", "readers", member, ["ops.alert"]),
        (failure: unknown) => {
          assert.equal((failure as RefusedError).kind, "subscription_id_taken");
          return true;
        },
      );
      await handle.announce("ops/2", ["ops.alert"], "Disk full on db-3");
      const pausedTemplate: any = await handle.announcementSubscriptionPause(
        "op-template-pause",
        "readers",
        template.revision,
      );
      assert.equal(pausedTemplate.subscription, null);
      assert.equal(pausedTemplate.template.state, "paused");
      assert.equal(pausedTemplate.children.length, 1);
      assert.ok(pausedTemplate.children[0].id.startsWith("readers/"));
      assert.equal(pausedTemplate.children[0].member.identity, "reader:1");
      const cancelledTemplate: any = await handle.announcementSubscriptionCancel(
        "op-template-cancel",
        "readers",
        pausedTemplate.template.revision,
      );
      assert.equal(cancelledTemplate.template.state, "cancelled");
      assert.deepEqual(cancelledTemplate.children.map((child: any) => child.state), ["cancelled"]);
    } finally {
      await runtime.shutdown();
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

// -- Handling, redelivery and dismissal ---------------------------------------

const HANDLING = {
  state: "unhandled",
  cause: "abandoned_with_error",
  redelivered: 0,
  held: 2,
  dismissed: 0,
  read_at_ms: 50,
};

describe("MobHandle handling, redelivery and dismissal", () => {
  const served: [string, (h: MobHandle) => Promise<unknown>, string, Record<string, unknown>][] = [
    ["announcementDeliveryRedeliver", (h) => (h as any).announcementDeliveryRedeliver("op-r", "batch-1"),
      "meta/mob_announcements/deliveries/redeliver", { operation_id: "op-r", batch_id: "batch-1" }],
    ["announcementDeliveryDismiss", (h) => (h as any).announcementDeliveryDismiss("op-d", "batch-1"),
      "meta/mob_announcements/deliveries/dismiss", { operation_id: "op-d", batch_id: "batch-1" }],
  ];
  for (const [name, call, method, params] of served) {
    it(`${name} sends ${method} with its params`, async () => {
      const { handle, calls } = mockHandle(() => ({ handling: HANDLING }));
      await call(handle);
      assert.deepEqual(calls, [{ method, params }]);
    });
  }

  it("redeliver and dismiss read the handling", async () => {
    const { handle } = mockHandle((method) =>
      method.endsWith("redeliver")
        ? { handling: { ...HANDLING, held: 0, redelivered: 2 } }
        : { handling: { ...HANDLING, held: 0, dismissed: 2 } });
    const redelivered = await (handle as any).announcementDeliveryRedeliver("op-r", "batch-1");
    assert.deepEqual(
      [redelivered.state, redelivered.cause, redelivered.redelivered, redelivered.held],
      ["unhandled", "abandoned_with_error", 2, 0],
    );
    const dismissed = await (handle as any).announcementDeliveryDismiss("op-d", "batch-1");
    assert.deepEqual([dismissed.dismissed, dismissed.readAtMs], [2, 50]);
  });

  it("a batch and its observation read the handling", async () => {
    let { handle } = mockHandle((method) =>
      method.endsWith("deliveries/list")
        ? {
            deliveries: [
              { ...BATCH, handling: HANDLING },
              { ...BATCH, handling: null },
              { ...BATCH, handling: { ...HANDLING, state: "a_future_state", cause: null } },
            ],
          }
        : { batch: BATCH, native: { status: "applied" }, handling: { ...HANDLING, state: "pending" } });
    const [handled, unread, future] = await handle.announcementDeliveryList();
    assert.deepEqual([(handled.handling as any).state, (handled.handling as any).held], ["unhandled", 2]);
    assert.equal(unread.handling, null);
    assert.equal((future.handling as any).state, "a_future_state");
    assert.equal((future.handling as any).cause, null);
    const observation = await handle.announcementDeliveryObserve("batch-1");
    assert.equal((observation as any).handling.state, "pending");
    ({ handle } = mockHandle(() => ({ batch: BATCH, native: { status: "not_submitted" } })));
    assert.equal((await handle.announcementDeliveryObserve("batch-1") as any).handling, null);
  });

  it("exports parseAnnouncementBatchHandling", () => {
    assert.equal(typeof api.parseAnnouncementBatchHandling, "function");
  });
});

// -- Members, escalations and status ----------------------------------------

const ESCALATION = {
  escalation_id: "esc-1",
  cause: "failure_streak",
  subject: { kind: "source", mob_id: "mob-a", source_id: "mail" },
  since_ms: 10,
  raised_at_ms: 20,
  renotify_count: 1,
};

const MEMBER_READ = {
  member: MEMBER,
  backlog: {
    eligible_count: 3,
    eligible_bytes: 120,
    oldest_captured_at_ms: 5,
    last_sealed_at_ms: null,
    outstanding: true,
    held_audience: 1,
    held_unhandled: 0,
    held_refused: 0,
    withheld_audience: 2,
  },
  marker: { cause: { kind: "member_not_live" }, evidence: "local", since_ms: 7 },
  escalations: [{ ...ESCALATION, escalation_id: "esc-2", cause: "suspended", subject: { kind: "member", member: MEMBER } }],
};

const ESCALATION_EVENT = {
  cursor: 4,
  escalation_id: "esc-1",
  event: "raised",
  cause: "failure_streak",
  subject: ESCALATION.subject,
  since_ms: 10,
  at_ms: 20,
  renotify_count: 0,
};

describe("MobHandle member, escalation and status reads", () => {
  const served: [string, (h: MobHandle) => Promise<unknown>, string, Record<string, unknown>][] = [
    ["announcementMemberGet", (h) => (h as any).announcementMemberGet(MEMBER),
      "meta/mob_announcements/members/get", { member: MEMBER }],
    ["announcementMemberList", (h) => (h as any).announcementMemberList({ after: MEMBER, limit: 5, escalated: true }),
      "meta/mob_announcements/members/list", { after: MEMBER, limit: 5, escalated: true }],
    ["announcementEscalationsList", (h) => (h as any).announcementEscalationsList({ after: 3, limit: 10 }),
      "meta/mob_announcements/escalations/list", { after: 3, limit: 10 }],
    ["announcementEscalationsList (open)", (h) => (h as any).announcementEscalationsList({ open: true }),
      "meta/mob_announcements/escalations/list", { open: true }],
    ["announcementSourceList (escalated)", (h) => h.announcementSourceList({ escalated: true } as any),
      "meta/mob_announcements/sources/list", { escalated: true }],
  ];
  for (const [name, call, method, params] of served) {
    it(`${name} sends ${method} with its params`, async () => {
      const { handle, calls } = mockHandle(() => ({}));
      await call(handle);
      assert.deepEqual(calls, [{ method, params }]);
    });
  }

  it("member get reads the backlog, the marker and the escalations", async () => {
    const { handle } = mockHandle(() => MEMBER_READ);
    const view = await (handle as any).announcementMemberGet(MEMBER);
    assert.equal(view.member.identity, "lead:main");
    assert.equal(view.marker?.causeKind, "member_not_live");
    assert.deepEqual(
      [view.backlog.eligibleCount, view.backlog.eligibleBytes, view.backlog.outstanding],
      [3, 120, true],
    );
    assert.deepEqual([view.backlog.heldAudience, view.backlog.withheldAudience], [1, 2]);
    assert.equal(view.backlog.lastSealedAtMs, null);
    assert.equal(view.backlog.oldestCapturedAtMs, 5);
    const [escalation] = view.escalations;
    assert.deepEqual([escalation.escalationId, escalation.cause, escalation.subjectKind], ["esc-2", "suspended", "member"]);
  });

  it("member list reads member views", async () => {
    const { handle } = mockHandle(() => ({ members: [MEMBER_READ, { ...MEMBER_READ, marker: null, escalations: [] }] }));
    const [first, second] = await (handle as any).announcementMemberList();
    assert.equal(first.escalations[0].cause, "suspended");
    assert.equal(second.marker, null);
    assert.deepEqual(second.escalations, []);
  });

  it("escalations list reads the events or the open escalations", async () => {
    let { handle } = mockHandle(() => ({
      events: [ESCALATION_EVENT, { ...ESCALATION_EVENT, cursor: 5, event: "a_future_event" }],
      next: 5,
    }));
    let page = await (handle as any).announcementEscalationsList({ after: 3 });
    assert.equal(page.next, 5);
    assert.equal(page.escalations, null);
    assert.deepEqual(page.events.map((event: any) => [event.cursor, event.event]), [[4, "raised"], [5, "a_future_event"]]);
    assert.equal(page.events[0].subjectKind, "source");
    assert.equal(page.events[0].subject.source_id, "mail");
    ({ handle } = mockHandle(() => ({
      escalations: [ESCALATION, { ...ESCALATION, cause: "a_future_cause", resolved_at_ms: 30 }],
      next: 9,
    })));
    page = await (handle as any).announcementEscalationsList({ open: true });
    assert.equal(page.next, 9);
    assert.equal(page.events, null);
    const [first, second] = page.escalations;
    assert.deepEqual([first.cause, first.renotifyCount, first.resolvedAtMs], ["failure_streak", 1, null]);
    assert.deepEqual([second.cause, second.resolvedAtMs], ["a_future_cause", 30]);
  });

  it("a source health reads its escalations and monitoring", async () => {
    const health = {
      ...SOURCE.health,
      escalations: [ESCALATION],
      runner_scheduled_here: false,
      quiet_monitoring: { configured: true, quiet_after_ms: 3600000, last_new_item_at_ms: 40 },
      empty_coverage_monitoring: { configured: false, empty_after_ms: null, empty_snapshot_since_ms: null },
    };
    const { handle } = mockHandle(() => ({ sources: [{ ...SOURCE, health }, SOURCE] }));
    const [escalated, plain] = await handle.announcementSourceList();
    const escalatedHealth = escalated.health as any;
    assert.equal(escalatedHealth.escalations[0].cause, "failure_streak");
    assert.equal(escalatedHealth.runnerScheduledHere, false);
    assert.equal(escalatedHealth.quietMonitoring.configured, true);
    assert.equal(escalatedHealth.emptyCoverageMonitoring.configured, false);
    const plainHealth = plain.health as any;
    assert.deepEqual(plainHealth.escalations, []);
    assert.equal(plainHealth.runnerScheduledHere, null);
    assert.equal(plainHealth.quietMonitoring, null);
  });

  for (const name of ["parseAnnouncementEscalation", "parseAnnouncementEscalationsPage", "parseAnnouncementMemberView"]) {
    it(`exports ${name}`, () => {
      assert.equal(typeof api[name], "function");
    });
  }

  it("status reads the mob announcement counts", () => {
    const data = {
      contract_version: "0.5.0",
      running: true,
      loaded_modules: [],
      mob_announcements: {
        sources: { active: 2, paused: 1, failing: 1, stale: 0 },
        members: { suspended: 1, unhandled_held: 0 },
        escalations: { open: 3, last_event_cursor: 12 },
      },
    };
    const counts = (api.parseStatusResult(data) as any).mobAnnouncements;
    assert.deepEqual(counts.sources, { active: 2, paused: 1, failing: 1, stale: 0 });
    assert.deepEqual(counts.members, { suspended: 1, unhandled_held: 0 });
    assert.deepEqual([counts.openEscalations, counts.lastEventCursor], [3, 12]);
    assert.equal((api.parseStatusResult({ ...data, mob_announcements: null }) as any).mobAnnouncements, null);
    const { mob_announcements: _omitted, ...absent } = data;
    assert.equal("mobAnnouncements" in api.parseStatusResult(absent), false);
  });
});

// -- Declarations and role templates ------------------------------------------

const ROLE = { mob_id: "mob-a", role: "reviewer" };

const TEMPLATE = {
  id: "reviewers",
  members: ROLE,
  topics: ["ops.alert"],
  mobs: ["mob-a"],
  state: "active",
  revision: 1,
  created_at_ms: 10,
  updated_at_ms: 10,
};

const DECLARATIONS = {
  revision: 3,
  subscriptions: [{ subscription_id: "reviewers", members: ROLE, topics: ["ops.alert"] }],
};

const DECLARATIONS_VIEW = {
  revision: 3,
  digest: "sha256:abc",
  records: [
    { kind: "source", id: "mail", status: "applied", collection: "live" },
    { kind: "subscription", id: "sub-1", status: "pending", refusal: { kind: "unknown_member", message: "no such member" } },
    {
      kind: "role_child",
      id: "reviewers/0123456789abcdef",
      status: "applied",
      delivery: { kind: "refused", cause: "governed_runtime" },
    },
    { kind: "a_future_kind", id: "x", status: "a_future_status" },
  ],
};

describe("MobHandle declarations and role templates", () => {
  const served: [string, (h: MobHandle) => Promise<unknown>, string, Record<string, unknown>][] = [
    ["announcementRoleTemplatePut", (h) => (h as any).announcementRoleTemplatePut("op-t", "reviewers", ROLE, ["ops.alert"]),
      "meta/mob_announcements/subscriptions/put",
      { operation_id: "op-t", subscription_id: "reviewers", members: ROLE, topics: ["ops.alert"] }],
    ["announcementRoleTemplatePut with options",
      (h) => (h as any).announcementRoleTemplatePut("op-t", "reviewers", ROLE, ["ops.alert"], { mobs: ["mob-a"], expectedRevision: 2 }),
      "meta/mob_announcements/subscriptions/put",
      { operation_id: "op-t", subscription_id: "reviewers", members: ROLE, topics: ["ops.alert"], mobs: ["mob-a"], expected_revision: 2 }],
    ["announcementDeclarationsApply", (h) => (h as any).announcementDeclarationsApply(DECLARATIONS),
      "meta/mob_announcements/declarations/apply", { revision: 3, declarations: DECLARATIONS }],
    ["announcementDeclarationsApply from a sidecar", (h) => (h as any).announcementDeclarationsApply(),
      "meta/mob_announcements/declarations/apply", {}],
    ["announcementDeclarationsGet", (h) => (h as any).announcementDeclarationsGet(),
      "meta/mob_announcements/declarations/get", {}],
  ];
  for (const [name, call, method, params] of served) {
    it(`${name} sends ${method} with its params`, async () => {
      const { handle, calls } = mockHandle(() => ({}));
      await call(handle);
      assert.deepEqual(calls, [{ method, params }]);
    });
  }

  it("a role template put reads the template", async () => {
    const { handle, calls } = mockHandle(() => ({ subscription: TEMPLATE }));
    const template = await (handle as any).announcementRoleTemplatePut(
      "op-t",
      "reviewers",
      { mobId: "mob-a", role: "reviewer" },
      ["ops.alert"],
    );
    assert.deepEqual((calls[0].params as any).members, ROLE);
    assert.deepEqual([template.id, template.members.mobId, template.members.role], ["reviewers", "mob-a", "reviewer"]);
    assert.deepEqual([template.topics, template.mobs], [["ops.alert"], ["mob-a"]]);
    assert.deepEqual(
      [template.state, template.revision, template.createdAtMs, template.updatedAtMs],
      ["active", 1, 10, 10],
    );
  });

  it("a template verb reads the template and its children", async () => {
    const child = { ...SUBSCRIPTION, id: "reviewers/0123456789abcdef", state: "paused" };
    const { handle } = mockHandle(() => ({
      template: { ...TEMPLATE, state: "paused", revision: 2 },
      children: [child],
      unresolved_batches: ["batch-1"],
    }));
    const change: any = await handle.announcementSubscriptionPause("op-p", "reviewers", 1);
    assert.equal(change.subscription, null);
    assert.deepEqual([change.template.state, change.template.revision], ["paused", 2]);
    assert.deepEqual(change.children.map((c: any) => [c.id, c.state]), [["reviewers/0123456789abcdef", "paused"]]);
    assert.deepEqual(change.unresolvedBatches, ["batch-1"]);
  });

  it("a subscription verb names no template", async () => {
    const { handle } = mockHandle(() => ({ subscription: { ...SUBSCRIPTION, state: "cancelled" }, unresolved_batches: [] }));
    const change: any = await handle.announcementSubscriptionCancel("op-c", "sub-1", 2);
    assert.equal(change.subscription.state, "cancelled");
    assert.equal(change.template, null);
    assert.deepEqual(change.children, []);
  });

  it("subscriptions/get reads a role template", async () => {
    let { handle } = mockHandle(() => ({ subscription: null, marker: null, template: TEMPLATE }));
    const view: any = await handle.announcementSubscriptionGet("reviewers");
    assert.equal(view.subscription, null);
    assert.equal(view.marker, null);
    assert.deepEqual([view.template.id, view.template.members.role], ["reviewers", "reviewer"]);
    ({ handle } = mockHandle(() => ({ subscription: SUBSCRIPTION, marker: null })));
    assert.equal(((await handle.announcementSubscriptionGet("sub-1")) as any).template, null);
  });

  it("declarations read their records", async () => {
    const { handle } = mockHandle((method) =>
      method.endsWith("declarations/apply") ? { ...DECLARATIONS_VIEW, revision: 4 } : DECLARATIONS_VIEW);
    const view = await (handle as any).announcementDeclarationsGet();
    assert.deepEqual([view.revision, view.digest], [3, "sha256:abc"]);
    const [source, pending, child, future] = view.records;
    assert.deepEqual([source.kind, source.id, source.status, source.collection], ["source", "mail", "applied", "live"]);
    assert.deepEqual([pending.status, pending.refusalKind], ["pending", "unknown_member"]);
    assert.deepEqual(pending.refusal, { kind: "unknown_member", message: "no such member" });
    assert.deepEqual([child.kind, child.deliveryKind, child.delivery.cause], ["role_child", "refused", "governed_runtime"]);
    assert.deepEqual(
      [future.kind, future.status, future.refusal, future.collection, future.delivery],
      ["a_future_kind", "a_future_status", null, null, null],
    );
    assert.equal((await (handle as any).announcementDeclarationsApply(DECLARATIONS)).revision, 4);
  });

  const refusals: [string, Record<string, unknown>][] = [
    ["declaration_rollback", { applied: 4 }],
    ["declaration_digest_conflict", { revision: 3 }],
    ["role_template_limit", { limit: 256 }],
    ["subscription_id_taken", { subscription_id: "reviewers" }],
  ];
  for (const [kind, data] of refusals) {
    it(`-32047 ${kind} keeps the refused class and its data`, () => {
      const reified = parseAnnouncementRpcError(-32047, "refused", "r-1", "meta/mob_announcements/declarations/apply", {
        kind,
        ...data,
      }) as RefusedError;
      assert.equal((reified as Error).constructor, errorClass("AnnouncementRefusedError"));
      assert.equal(reified.kind, kind);
      for (const [key, value] of Object.entries(data)) {
        assert.equal(((reified as any).data as Record<string, unknown>)[key], value);
      }
    });
  }

  it("exports the declaration and template parsers", () => {
    for (const name of [
      "parseAnnouncementRoleSelector",
      "parseAnnouncementRoleTemplate",
      "parseAnnouncementDeclarationRecord",
      "parseAnnouncementDeclarationsView",
    ]) {
      assert.equal(typeof api[name], "function", name);
    }
  });
});

// -- Handling reads and awaiting handling ------------------------------------

const HANDLING_READ = {
  pending: 1,
  handled: 4,
  handled_unverified: 2,
  unhandled: 1,
  held: 3,
  oldest_unhandled_at_ms: 70,
};

describe("MobHandle handling reads and awaiting handling", () => {
  it("announcementSubscriptionPut sends delivery only when set", async () => {
    let { handle, calls } = mockHandle(() => ({}));
    await (handle as any).announcementSubscriptionPut("op-s", "sub-1", MEMBER, ["ops.alert"], {
      delivery: { await_handling: true },
    });
    assert.deepEqual((calls[0].params as any).delivery, { await_handling: true });
    ({ handle, calls } = mockHandle(() => ({})));
    await handle.announcementSubscriptionPut("op-s", "sub-1", MEMBER, ["ops.alert"]);
    assert.equal("delivery" in (calls[0].params as any), false);
  });

  it("a subscription reads its delivery", async () => {
    let { handle } = mockHandle(() => ({
      subscription: { ...SUBSCRIPTION, delivery: { await_handling: false } },
      marker: null,
    }));
    let subscription: any = (await handle.announcementSubscriptionGet("sub-1")).subscription;
    assert.deepEqual([subscription.delivery, subscription.awaitHandling], [{ await_handling: false }, false]);
    ({ handle } = mockHandle(() => ({ subscription: SUBSCRIPTION, marker: null })));
    subscription = (await handle.announcementSubscriptionGet("sub-1")).subscription;
    assert.deepEqual([subscription.delivery, subscription.awaitHandling], [null, null]);
  });

  it("member get reads its handling", async () => {
    let { handle } = mockHandle(() => ({ ...MEMBER_READ, handling: HANDLING_READ }));
    let handling = ((await (handle as any).announcementMemberGet(MEMBER)) as any).handling;
    assert.deepEqual(
      [handling.pending, handling.handled, handling.handledUnverified, handling.unhandled, handling.held],
      [1, 4, 2, 1, 3],
    );
    assert.equal(handling.oldestUnhandledAtMs, 70);
    ({ handle } = mockHandle(() => ({ ...MEMBER_READ, handling: { ...HANDLING_READ, oldest_unhandled_at_ms: null } })));
    handling = ((await (handle as any).announcementMemberGet(MEMBER)) as any).handling;
    assert.equal(handling.oldestUnhandledAtMs, null);
    ({ handle } = mockHandle(() => MEMBER_READ));
    assert.equal(((await (handle as any).announcementMemberGet(MEMBER)) as any).handling, null);
  });

  it("status reads the handling counts", () => {
    const announcements = {
      sources: { active: 1, paused: 0, failing: 0, stale: 0 },
      members: { suspended: 0, unhandled_held: 5 },
      handling: { pending: 2, handled: 9, handled_unverified: 1, unhandled: 3 },
      escalations: { open: 1, last_event_cursor: 4 },
    };
    const data = { contract_version: "0.5.0", running: true, loaded_modules: [], mob_announcements: announcements };
    const counts = (api.parseStatusResult(data) as any).mobAnnouncements;
    assert.equal(counts.members.unhandled_held, 5);
    assert.deepEqual(counts.handling, { pending: 2, handled: 9, handled_unverified: 1, unhandled: 3 });
    const { handling: _omitted, ...older } = announcements;
    assert.deepEqual((api.parseStatusResult({ ...data, mob_announcements: older }) as any).mobAnnouncements.handling, {});
  });

  for (const cause of ["unhandled", "handled_unverified"]) {
    it(`a ${cause} escalation reads as sent`, async () => {
      const { handle } = mockHandle(() => ({
        ...MEMBER_READ,
        escalations: [{ ...ESCALATION, cause, subject: { kind: "member", member: MEMBER } }],
      }));
      const [escalation] = ((await (handle as any).announcementMemberGet(MEMBER)) as any).escalations;
      assert.deepEqual([escalation.cause, escalation.subjectKind], [cause, "member"]);
    });
  }

  it("exports parseAnnouncementMemberHandling", () => {
    assert.equal(typeof api.parseAnnouncementMemberHandling, "function");
  });
});

// -- Template deliveries -------------------------------------------------------

describe("MobHandle template deliveries", () => {
  it("announcementRoleTemplatePut sends delivery only when set", async () => {
    let { handle, calls } = mockHandle(() => ({}));
    await (handle as any).announcementRoleTemplatePut("op-t", "reviewers", ROLE, ["ops.alert"], {
      delivery: { await_handling: false },
    });
    assert.deepEqual((calls[0].params as any).delivery, { await_handling: false });
    ({ handle, calls } = mockHandle(() => ({})));
    await (handle as any).announcementRoleTemplatePut("op-t", "reviewers", ROLE, ["ops.alert"]);
    assert.equal("delivery" in (calls[0].params as any), false);
  });

  it("a role template reads its delivery", async () => {
    let { handle } = mockHandle(() => ({
      subscription: null,
      marker: null,
      template: { ...TEMPLATE, delivery: { await_handling: true } },
    }));
    let template: any = ((await handle.announcementSubscriptionGet("reviewers")) as any).template;
    assert.deepEqual([template.delivery, template.awaitHandling], [{ await_handling: true }, true]);
    ({ handle } = mockHandle(() => ({ subscription: TEMPLATE })));
    template = await (handle as any).announcementRoleTemplatePut("op-t", "reviewers", ROLE, ["ops.alert"]);
    assert.deepEqual([template.delivery, template.awaitHandling], [null, null]);
  });
});
