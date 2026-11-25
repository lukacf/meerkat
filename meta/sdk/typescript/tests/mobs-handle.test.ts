/**
 * A mob handle names its mob on the wire; the handle of a launch's own mob
 * names none.
 *
 * `runtime.mobHandle()` is the handle of the mob a launch names. A request it
 * sends carries a top-level `mob_id` only when the caller passed one to a
 * method that takes it; the handle adds none of its own. Its JSON-RPC
 * requests leave through one place, `MetaRuntime._rpc`, except the two it
 * sends as a multipart upload over HTTP, and its two event streams are HTTP
 * reads.
 *
 * `runtime.mob(mobId)` is the same handle for a mob named by its id. It adds
 * that id in each of those three places: to every JSON-RPC request, to the
 * multipart payload, and to the two stream URLs. A gateway that does not
 * advertise `mob_addressing` would ignore the id and answer for its own mob,
 * so the handle reads the capability first (once per started transport) and
 * refuses locally without it. `runtime.mobs` creates, lists and reads the
 * mobs of the host and keeps nothing between calls.
 *
 * A handle also offers the runtime's calls by identity for its mob: under the
 * runtime's names where the handle has none of that name, as `sendIdentity`
 * and `statusIdentity` where it has, and through `identities`.
 *
 * These tests hold that for every public name of `MobHandle`: a method added
 * later is driven here with a text for each parameter it declares, or fails
 * here until it is given arguments.
 */

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import * as sdk from "../src/index.js";
import {
  CapabilityUnavailableError,
  ContractMismatchError,
  RpcError,
  StorageResolutionError,
} from "../src/errors.js";
import {
  parseActiveLiveChannelHandle,
  parseLivePlaybackOwnerReadiness,
  parsePendingLiveChannelHandle,
} from "../src/live.js";
import { IdentitiesHandle, MetaRuntime, MobHandle, SseBridge } from "../src/runtime.js";
import { runtimeConfig } from "./runtime-config.js";

// Every wire method `MobHandle` sends. A method that sends a new one adds it
// here; the last test fails while this list and the handle differ.
const WIRE_METHODS: ReadonlySet<string> = new Set(
  `
  live/webrtc/answer
  meta/agent_definitions/list
  meta/agent_memory/forget meta/agent_memory/manifest meta/agent_memory/recall
  meta/agent_memory/remember meta/agent_memory/update
  meta/attach_existing_session
  meta/blob/get meta/blob/upload
  meta/call_tool meta/cancel_flow meta/capabilities meta/collect_completed
  meta/cross_mob/directory meta/cross_mob/peer_info
  meta/cross_mob/unwire_local meta/cross_mob/wire_local
  meta/decisions/decide meta/decisions/get meta/decisions/list
  meta/decisions/present meta/decisions/rendering meta/decisions/withdraw
  meta/delivery/history meta/delivery/send
  meta/ensure_member meta/events/subscribe meta/find_members meta/flow_status
  meta/force_cancel_member meta/fork_helper
  meta/gating/audit meta/gating/decide meta/gating/evaluate
  meta/gating/pending meta/gating/questions meta/gating/withdraw
  meta/get_member
  meta/identity/resolved_tools meta/identity/routing_status meta/identity/warm
  meta/list_flows meta/list_members meta/list_runs
  meta/live/close meta/live/commit_input meta/live/interrupt meta/live/open
  meta/live/playback_complete meta/live/playback_owner/register
  meta/live/playback_owner/revoke meta/live/refresh
  meta/live/replacement_required meta/live/send_input meta/live/status
  meta/live/truncate
  meta/member_health meta/member_status
  meta/memory/index meta/memory/query meta/memory/stores
  meta/mob_announcements/announce meta/mob_announcements/audit/list
  meta/mob_announcements/declarations/apply
  meta/mob_announcements/declarations/get
  meta/mob_announcements/deliveries/dismiss
  meta/mob_announcements/deliveries/list
  meta/mob_announcements/deliveries/observe
  meta/mob_announcements/deliveries/reconcile
  meta/mob_announcements/deliveries/redeliver
  meta/mob_announcements/escalations/list
  meta/mob_announcements/get
  meta/mob_announcements/members/get meta/mob_announcements/members/list
  meta/mob_announcements/sources/advance meta/mob_announcements/sources/end
  meta/mob_announcements/sources/get meta/mob_announcements/sources/list
  meta/mob_announcements/sources/pause meta/mob_announcements/sources/put
  meta/mob_announcements/sources/record_failure
  meta/mob_announcements/sources/resume
  meta/mob_announcements/subscriptions/cancel
  meta/mob_announcements/subscriptions/get
  meta/mob_announcements/subscriptions/list
  meta/mob_announcements/subscriptions/pause
  meta/mob_announcements/subscriptions/put
  meta/mob_announcements/subscriptions/resume
  meta/mob_events/query meta/mob_events/subscribe
  meta/mob_labels/delete meta/mob_labels/get meta/mob_labels/set
  meta/mobpacks/apply_operation meta/mobpacks/catalogs meta/mobpacks/create
  meta/mobpacks/delete meta/mobpacks/deploy meta/mobpacks/deploy_command
  meta/mobpacks/export meta/mobpacks/get meta/mobpacks/import
  meta/mobpacks/list meta/mobpacks/redo meta/mobpacks/save
  meta/mobpacks/source meta/mobpacks/templates meta/mobpacks/undo
  meta/mobpacks/validate
  meta/models/catalog meta/peer_pubkey meta/query_events
  meta/reconcile meta/reconcile_edges meta/rediscover
  meta/reload_member meta/respawn_member meta/retire_member
  meta/routing/resolve meta/routing/routes/add meta/routing/routes/delete
  meta/routing/routes/list
  meta/run_flow
  meta/run_labels/delete meta/run_labels/get meta/run_labels/set
  meta/schedule/tools meta/send_message meta/session_store/bigquery
  meta/skills/catalog meta/spawn_helper meta/spawn_member meta/status
  meta/stop_member_run meta/storage/doctor meta/tools/catalog meta/wait_ready
  meta/workgraph/attention/list meta/workgraph/attention/pause
  meta/workgraph/attention/prune meta/workgraph/attention/reassign
  meta/workgraph/attention/resume
  meta/workgraph/block meta/workgraph/claim meta/workgraph/close
  meta/workgraph/create meta/workgraph/events meta/workgraph/evidence/add
  meta/workgraph/get
  meta/workgraph/goal/confirm meta/workgraph/goal/create
  meta/workgraph/goal/request_close meta/workgraph/goal/status
  meta/workgraph/link meta/workgraph/list meta/workgraph/policy/escalate
  meta/workgraph/ready meta/workgraph/release meta/workgraph/snapshot
  meta/workgraph/update
  `
    .split(/\s+/)
    .filter((name) => name !== ""),
);

// The wire methods of the runtime's calls by identity, which a handle sends
// for its mob as well.
const IDENTITY_FIRST_WIRE_METHODS: ReadonlySet<string> = new Set(
  `
  meta/compact_member meta/completion_cursor meta/delete_identity meta/dispatch
  meta/inspect_identity meta/instruction_status
  meta/member_activate_instruction meta/member_instruction_activations
  meta/prune_member_system_rows meta/reconcile_identity meta/recover_delivery
  meta/registered_spec meta/reset meta/respawn meta/retire
  meta/revoke_instruction meta/send meta/send_status meta/set_instruction
  meta/status_identity meta/status_identity_bootstrap meta/subscribe
  meta/tool_grants/audit meta/tool_grants/grant meta/tool_grants/list
  meta/tool_grants/revoke meta/turn_result meta/wait_for_completion
  meta/wait_for_turn meta/wait_identity_bootstrap
  `
    .split(/\s+/)
    .filter((name) => name !== ""),
);

// The runtime's calls by identity that a handle offers under the runtime's
// own name ...
const FORWARDED = [
  "agent",
  "sendStatus",
  "dispatch",
  "recoverDelivery",
  "turnResult",
  "waitForTurn",
  "completionCursor",
  "waitForCompletion",
  "waitUntilReady",
  "sendAndWait",
  "sendAndWaitOutcome",
  "dispatchAndWait",
  "dispatchAndWaitOutcome",
  "subscribe",
  "registeredSpec",
  "inspectIdentity",
  "respawn",
  "retire",
  "reset",
  "deleteIdentity",
  "compactMember",
  "reconcileIdentity",
  "identityBootstrapStatus",
  "waitIdentityBootstrap",
  "activateMemberInstruction",
  "memberInstructionActivations",
  "setInstruction",
  "revokeInstruction",
  "instructionStatus",
  "pruneMemberSystemRows",
  "grantTool",
  "revokeToolGrant",
  "listToolGrants",
  "toolGrantAudit",
] as const;
// ... and the two whose name the handle has already for another call: the
// handle's name for the runtime's call, the runtime's name, and the wire
// method of each of the two calls that share the runtime's name.
const RENAMED: Readonly<Record<string, readonly [string, string, string]>> = {
  sendIdentity: ["send", "meta/send", "meta/send_message"],
  statusIdentity: ["status", "meta/status_identity", "meta/status"],
};
// What stays the runtime's alone: its own lifecycle and wiring, the calls
// that are the host's whichever mob is named, and a helper of its own.
const RUNTIME_ONLY: readonly string[] = [
  "connect",
  "shutdown",
  "lastGatewayTermination",
  "setRustHttpBase",
  "onDecisionNotice",
  "onScheduleFire",
  "sseBridge",
  "mobHandle",
  "mob",
  "realmMcpServers",
  "realmMcpInstall",
  "realmMcpSetSecret",
  "realmMcpRemoveSecret",
  "requireKeyedSend",
];

// The wire methods that leave as a multipart upload over HTTP: a blob upload
// always, a message when it carries attachments.
const MULTIPART_ALWAYS: ReadonlySet<string> = new Set(["meta/blob/upload"]);
const MULTIPART_WITH_ATTACHMENTS: ReadonlySet<string> = new Set(["meta/send_message"]);

// Public names that send nothing themselves.
const SENDS_NOTHING: ReadonlySet<string> = new Set(["toolCaller"]);

// The two event streams and the HTTP path each reads.
const STREAM_PATHS: Readonly<Record<string, string>> = {
  subscribeAgent: "/agents/x/events",
  subscribeMob: "/mob/events",
};

// The methods with a `mobId` of their own: the mob an announcement, a source
// or an audit page belongs to. `announcementGet` takes it first and requires
// it; the others take it in their options and send it only when given.
const OWN_MOB_ID_REQUIRED: ReadonlySet<string> = new Set(["announcementGet"]);
const OWN_MOB_ID_OPTIONAL: ReadonlySet<string> = new Set([
  "announce",
  "announcementSourcePut",
  "announcementSourcePause",
  "announcementSourceResume",
  "announcementSourceEnd",
  "announcementSourceGet",
  "announcementSourceList",
  "announcementSourceAdvance",
  "announcementSourceRecordFailure",
  "announcementAuditList",
  "queryEvents",
]);
// Of those, the calls whose mob is the mob the request is answered for and
// nothing an earlier gateway reads so: the handle of the launch's own mob
// reads the capability before it sends one.
const OWN_MOB_ID_NEEDS_THE_CAPABILITY: ReadonlySet<string> = new Set(["queryEvents"]);

const HTTP_BASE = "http://127.0.0.1:9";

type Sent = { method: string; params: Record<string, unknown> };
type Args = (handle: MobHandle) => unknown[];
type Callable = (...args: unknown[]) => unknown;

const LIVE = JSON.parse(
  readFileSync(
    new URL("../../../crates/meerkat-meta/tests/fixtures/live_contracts_v1.json", import.meta.url),
    "utf8",
  ),
) as Record<string, unknown>;
const pending = () => parsePendingLiveChannelHandle(LIVE.pending_channel_handle);
const active = () => parseActiveLiveChannelHandle(LIVE.active_channel_handle);
const readiness = () => parseLivePlaybackOwnerReadiness(LIVE.playback_owner_readiness);
const output = () => ({ channelId: active().channelId, outputId: "output-1", contentIndex: 0 });
const member = { mobId: "mob-a", identity: "worker-1" };
const png = () => new Blob(["png"], { type: "image/png" });
const dispatchInput = { content: "hello", origin: "system" };
const deliveryScope = {
  version: 1,
  identity: "worker-1",
  agentRuntimeId: "runtime-1",
  generation: 1,
  leaseFencingToken: 1,
  member: {},
  sessionId: null,
};
/** A local media owner that prepares an offer and does nothing else. */
const playbackOwner = {
  prepare: async () => "offer",
  acceptAnswer: async () => {},
  activate: async () => {},
  abort: async () => {},
};

// Arguments a text per declared parameter does not serve: a value a method
// reads into before it sends.
const ARGS: Readonly<Record<string, Args>> = {
  announcementMemberGet: () => [member],
  announcementRoleTemplatePut: () => ["x", "x", { mobId: "mob-a", role: "worker" }, ["topic"]],
  announcementSubscriptionPut: () => ["x", "x", member, ["topic"]],
  liveCommitInputActive: () => [active()],
  liveConnect: () => ["x", { profileId: "profile-1" }, playbackOwner],
  liveExperimentalStatus: () => [pending()],
  liveInterruptActive: () => [active()],
  liveOutputs: () => [active()],
  livePlaybackComplete: () => [active(), output()],
  livePlaybackOwnerRegister: () => [pending()],
  livePlaybackOwnerRevoke: () => [pending(), readiness()],
  liveRefreshActive: () => [active()],
  liveReplacementRequired: () => [active()],
  liveSendInputImageActive: () => [active(), "x", "image/png", "eA=="],
  liveTruncate: () => [active(), output(), 1],
  liveWebrtcAnswerPending: () => [pending(), readiness(), "offer"],
  sendCrossMob: (handle) => ["x", handle, "hello"],
  sendDelivery: () => [{}],
  sessionStoreBigQuery: () => [{}],
  uploadBlob: () => [png()],
  upload_blob: () => [png()],
  dispatch: () => ["x", dispatchInput],
  dispatchAndWait: () => ["x", dispatchInput],
  dispatchAndWaitOutcome: () => ["x", dispatchInput],
  grantTool: () => ["x", { tool: "x" }],
  // The query is the call's one argument, and optional: none by default, so
  // that a `mobId` of its own is given in the query itself.
  queryEvents: () => [],
  gatingAnswer: () => [
    "x",
    { approverId: "x", presentedDigest: "x", answers: [{ questionId: "q-1", answer: "yes" }] },
  ],
  waitUntilReady: () => [["worker-1"]],
  recoverDelivery: () => ["x", deliveryScope, { idempotencyKey: "x", correlationId: "x" }],
};

// A second call of a method whose requests leave another way with other
// arguments.
const VARIANTS: Readonly<Record<string, Args>> = {
  send: () => ["x", "hello", { attachments: [png()] }],
};

/**
 * A text for each parameter a call declares. A call the handle forwards
 * declares what the runtime's call of that name does.
 */
function textArgs(name: string, method: Callable): unknown[] {
  const runtimeName =
    RENAMED[name]?.[0] ?? ((FORWARDED as readonly string[]).includes(name) ? name : undefined);
  const declared =
    runtimeName === undefined
      ? method.length
      : (MetaRuntime.prototype as unknown as Record<string, Callable>)[runtimeName].length;
  return Array.from({ length: declared }, () => "x");
}

/** What one call of one handle method sent. */
interface Run {
  /** Every request the runtime's transport was given. */
  transport: Sent[];
  /** The transport timeout each of those requests was given (undefined: the transport's own). */
  timeouts: Array<number | undefined>;
  /** The method of every request that passed `MetaRuntime._rpc`. */
  seam: string[];
  /** Every JSON-RPC request posted as a multipart upload. */
  multipart: Sent[];
  /** Every event stream URL opened. */
  streams: string[];
  /** Every consumer of live outputs the call registered. */
  liveOutputConsumers: Array<(output: unknown) => void>;
  /** What the call threw first, if it threw. */
  error?: unknown;
}

function newRun(): Run {
  return {
    transport: [],
    timeouts: [],
    seam: [],
    multipart: [],
    streams: [],
    liveOutputConsumers: [],
  };
}

/** How the recording transport answers. */
interface Gateway {
  /** The feature capabilities `meta/capabilities` advertises; none listed when absent. */
  readonly features?: readonly string[];
  /** A method's own answer. */
  readonly answers?: Readonly<Record<string, { result: unknown } | { error: unknown }>>;
}

function asSent(request: unknown): Sent {
  // As the wire carries it: a member that is undefined is not sent.
  const wire = JSON.parse(JSON.stringify(request)) as Record<string, unknown>;
  return {
    method: String(wire.method),
    params: (wire.params ?? {}) as Record<string, unknown>,
  };
}

/** A transport that records every request into `sent` and answers as `gateway` says. */
function recordingTransport(
  sent: Sent[],
  gateway: Gateway,
  timeouts: Array<number | undefined> = [],
): Record<string, unknown> {
  return {
    sendAsync: async (request: Record<string, unknown>, options?: { timeoutMs?: number }) => {
      await Promise.resolve();
      sent.push(asSent(request));
      timeouts.push(options?.timeoutMs);
      const method = String(request.method);
      const own = gateway.answers?.[method];
      if (own !== undefined) return { jsonrpc: "2.0", id: request.id, ...own };
      const result =
        method === "meta/capabilities" && gateway.features !== undefined
          ? {
              contract_version: "0.9.15",
              methods: [],
              loaded_modules: [],
              feature_capabilities: [...gateway.features],
            }
          : {};
      return { jsonrpc: "2.0", id: request.id, result };
    },
  };
}

function newRuntime(run: Run, gateway: Gateway = {}): MetaRuntime {
  const rt = new MetaRuntime(runtimeConfig());
  const internals = rt as unknown as Record<string, unknown>;
  internals._running = true;
  internals._transport = recordingTransport(run.transport, gateway, run.timeouts);
  rt.setRustHttpBase(HTTP_BASE);
  const throughRuntime = (internals._rpc as Callable).bind(rt);
  internals._rpc = async (method: string, ...rest: unknown[]) => {
    run.seam.push(method);
    return throughRuntime(method, ...rest);
  };
  const register = (internals._registerLiveOutputConsumer as Callable).bind(rt);
  internals._registerLiveOutputConsumer = (
    channelId: string,
    consumer: (output: unknown) => void,
  ) => {
    run.liveOutputConsumers.push(consumer);
    return register(channelId, consumer);
  };
  return rt;
}

function publicNames(): string[] {
  const handle = newRuntime(newRun()).mobHandle();
  const names = new Set<string>();
  for (const name of Object.getOwnPropertyNames(MobHandle.prototype)) names.add(name);
  // An alias a handle holds as its own member (`sendMessage`).
  for (const name of Object.getOwnPropertyNames(handle)) names.add(name);
  const members = handle as unknown as Record<string, unknown>;
  return [...names]
    .filter((name) => name !== "constructor" && !name.startsWith("_"))
    .filter((name) => typeof members[name] === "function")
    .sort();
}

const SETTLE_MS = 200;

/** Wait for `work`, or for long enough that whatever it sends first has left. */
async function settle(run: Run, work: Promise<unknown>): Promise<void> {
  let timer: NodeJS.Timeout | undefined;
  const waited = new Promise<void>((resolve) => {
    timer = setTimeout(resolve, SETTLE_MS);
  });
  try {
    await Promise.race([work.then(() => undefined), waited]);
  } catch (error) {
    // An empty answer is refused after the request left.
    run.error ??= error;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Call `MobHandle.<name>` once and return what it sent.
 *
 * The handle is the default one, or with `mobId` the handle of that mob. The
 * transport answers every request with an empty result, so a method may
 * throw on the answer; what it sent before that is what is asked here.
 */
async function drive(
  name: string,
  args?: Args,
  options: { readonly mobId?: string; readonly gateway?: Gateway } = {},
): Promise<Run> {
  const run = newRun();
  const rt = newRuntime(run, options.gateway);
  const handle = options.mobId === undefined ? rt.mobHandle() : rt.mob(options.mobId);
  const method = (handle as unknown as Record<string, Callable>)[name];
  const given =
    args !== undefined
      ? args(handle)
      : ARGS[name] !== undefined
        ? ARGS[name](handle)
        : textArgs(name, method);

  const originalFetch = globalThis.fetch;
  const bridge = SseBridge.prototype as unknown as Record<string, unknown>;
  const originalStream = bridge._fetchSseStream;
  globalThis.fetch = (async (url: string | URL | Request, init?: RequestInit) => {
    assert.equal(new URL(String(url)).pathname, "/console/rpc/multipart");
    const payload = JSON.parse(String((init?.body as FormData).get("payload")));
    run.multipart.push(asSent(payload));
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: payload.id, result: {} }), {
      headers: { "content-type": "application/json" },
    });
  }) as typeof fetch;
  bridge._fetchSseStream = async (url: string) => {
    run.streams.push(url);
    return { body: (async function* () {})(), destroy: () => {} };
  };
  try {
    let result: unknown;
    try {
      result = method.apply(handle, given);
    } catch (error) {
      run.error = error;
      return run;
    }
    const generator = result as Partial<AsyncGenerator<unknown>>;
    if (typeof generator?.next === "function" && typeof generator.return === "function") {
      const first = settle(run, generator.next());
      await new Promise((resolve) => setImmediate(resolve));
      // A stream that waits for a live output is given one, so it can be closed.
      for (const consume of run.liveOutputConsumers) consume(output());
      await first;
      await settle(run, generator.return(undefined));
    } else {
      await settle(run, Promise.resolve(result));
    }
  } finally {
    globalThis.fetch = originalFetch;
    bridge._fetchSseStream = originalStream;
  }
  return run;
}

function requests(run: Run): Sent[] {
  return [...run.transport, ...run.multipart];
}

const NAMES = publicNames();
const CASES: ReadonlyArray<{ id: string; name: string; args?: Args }> = [
  ...NAMES.map((name) => ({ id: name, name })),
  ...Object.keys(VARIANTS)
    .sort()
    .map((name) => ({ id: `${name} (variant)`, name, args: VARIANTS[name] })),
];

describe("the handle of a launch's own mob names no mob on the wire", () => {
  it("the tables name only methods the handle has", () => {
    const listed = [
      ...SENDS_NOTHING,
      ...Object.keys(STREAM_PATHS),
      ...OWN_MOB_ID_REQUIRED,
      ...OWN_MOB_ID_OPTIONAL,
      ...Object.keys(ARGS),
      ...Object.keys(VARIANTS),
    ];
    assert.deepEqual(
      listed.filter((name) => !NAMES.includes(name)),
      [],
    );
    // Every method that reads a `mobId` of its own is listed, and no other.
    const members = MobHandle.prototype as unknown as Record<string, Callable>;
    assert.deepEqual(
      NAMES.filter((name) => /\bmobId\b/.test(String(members[name] ?? ""))),
      [...OWN_MOB_ID_REQUIRED, ...OWN_MOB_ID_OPTIONAL].sort(),
    );
  });

  for (const { id, name, args } of CASES) {
    it(`the default handle adds no mob_id of its own: ${id}`, async () => {
      const run = await drive(name, args);

      if (SENDS_NOTHING.has(name)) {
        assert.deepEqual([requests(run), run.streams], [[], []]);
        return;
      }
      if (name in STREAM_PATHS) {
        assert.deepEqual(requests(run), []);
        assert.deepEqual(run.streams, [HTTP_BASE + STREAM_PATHS[name]]);
        // The stream's URL names no mob.
        assert.equal(new URL(run.streams[0]).search, "");
        return;
      }

      assert.notEqual(
        requests(run).length,
        0,
        `MobHandle.${name} sent nothing: give it arguments in ARGS`,
      );
      assert.deepEqual(run.streams, []);
      for (const { method, params } of requests(run)) {
        if (OWN_MOB_ID_REQUIRED.has(name)) {
          // The caller's own argument, as given.
          assert.equal(params.mob_id, "x", method);
        } else {
          assert.equal("mob_id" in params, false, `${method} ${JSON.stringify(params)}`);
        }
      }
    });
  }

  for (const name of [...OWN_MOB_ID_REQUIRED, ...OWN_MOB_ID_OPTIONAL].sort()) {
    it(`a method's own mobId is sent as the caller gave it: ${name}`, async () => {
      const asksFirst = OWN_MOB_ID_NEEDS_THE_CAPABILITY.has(name);
      const run = await drive(
        name,
        (handle) => {
          const method = (handle as unknown as Record<string, Callable>)[name];
          const base = ARGS[name]?.(handle) ?? textArgs(name, method);
          if (OWN_MOB_ID_REQUIRED.has(name)) return ["mob-b", ...base.slice(1)];
          // The options follow the parameters the method requires.
          return [...base.slice(0, method.length), { mobId: "mob-b" }];
        },
        asksFirst ? { gateway: { features: ["mob_addressing"] } } : {},
      );

      const sent = asksFirst ? addressed(run) : requests(run);
      assert.notEqual(sent.length, 0);
      for (const { method, params } of sent) {
        assert.equal(params.mob_id, "mob-b", method);
      }
    });
  }

  it("every wire method leaves through the one seam", async () => {
    const throughSeam = new Set<string>();
    const byMultipart = new Set<string>();
    for (const { name, args } of CASES) {
      const run = await drive(name, args);
      // Whatever the transport was given passed MetaRuntime._rpc, in order.
      assert.deepEqual(
        run.transport.map((sent) => sent.method),
        run.seam,
        name,
      );
      for (const method of run.seam) throughSeam.add(method);
      for (const sent of run.multipart) byMultipart.add(sent.method);
    }

    assert.deepEqual(
      [...byMultipart].sort(),
      [...MULTIPART_ALWAYS, ...MULTIPART_WITH_ATTACHMENTS].sort(),
    );
    assert.deepEqual(
      [...throughSeam].sort(),
      [...WIRE_METHODS, ...IDENTITY_FIRST_WIRE_METHODS]
        .filter((method) => !MULTIPART_ALWAYS.has(method))
        .sort(),
    );
    assert.equal(WIRE_METHODS.size, 161);
    assert.equal(IDENTITY_FIRST_WIRE_METHODS.size, 30);
    for (const method of IDENTITY_FIRST_WIRE_METHODS) assert.equal(WIRE_METHODS.has(method), false);
  });
});

// The feature capability of a gateway that honours `mob_id`, and the one of a
// gateway that serves the mobs of its host.
const MOB_ADDRESSING = "mob_addressing";
const HOSTED_MOBS = "hosted_mobs";
// The capability read: the host's own answer, so it names no mob.
const CAPABILITY_READ: Sent = { method: "meta/capabilities", params: {} };

/** The requests of a named handle's call, after its capability read. */
function addressed(run: Run): Sent[] {
  assert.deepEqual(run.transport.slice(0, 1), [CAPABILITY_READ]);
  return [...run.transport.slice(1), ...run.multipart];
}

/** The arguments of a call that names a mob of its own. */
function withOwnMobId(name: string, mobId: string): Args {
  return (handle) => {
    const method = (handle as unknown as Record<string, Callable>)[name];
    const base = ARGS[name]?.(handle) ?? textArgs(name, method);
    if (OWN_MOB_ID_REQUIRED.has(name)) return [mobId, ...base.slice(1)];
    // The options follow the parameters the method requires.
    return [...base.slice(0, method.length), { mobId }];
  };
}

describe("a handle for a mob named by its id", () => {
  it("obtaining a handle sends nothing", () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING, HOSTED_MOBS] });

    const handle = rt.mob("mob-b");
    const mobs = rt.mobs;

    assert.deepEqual([run.transport, run.multipart, run.streams], [[], [], []]);
    assert.ok(mobs instanceof sdk.MobsHandle);
    // The handle of a named mob is the handle a launch's own mob has, with an id.
    assert.equal(Object.getPrototypeOf(handle), MobHandle.prototype);
    assert.equal(handle.mobId, "mob-b");
    assert.equal(rt.mobHandle().mobId, null);
    // A mob is named by text.
    for (const notAnId of [undefined, null, 7]) {
      assert.throws(() => rt.mob(notAnId as unknown as string), TypeError);
    }
  });

  for (const { id, name, args } of CASES) {
    it(`a named handle sends its mob id on every call: ${id}`, async () => {
      const run = await drive(name, args, {
        mobId: "mob-b",
        gateway: { features: [MOB_ADDRESSING] },
      });

      if (SENDS_NOTHING.has(name)) {
        assert.deepEqual([requests(run), run.streams], [[], []]);
        return;
      }
      const sent = addressed(run);
      if (name in STREAM_PATHS) {
        assert.deepEqual(sent, []);
        assert.deepEqual(run.streams, [`${HTTP_BASE}${STREAM_PATHS[name]}?mob_id=mob-b`]);
        return;
      }

      assert.notEqual(sent.length, 0, `MobHandle.${name} sent nothing: give it arguments in ARGS`);
      assert.deepEqual(run.streams, []);
      // `announcementGet` is given a mob of its own here; that one is sent.
      const expected = OWN_MOB_ID_REQUIRED.has(name) ? "x" : "mob-b";
      for (const { method, params } of sent) {
        assert.equal(params.mob_id, expected, `${method} ${JSON.stringify(params)}`);
      }
    });
  }

  for (const name of [...OWN_MOB_ID_REQUIRED, ...OWN_MOB_ID_OPTIONAL].sort()) {
    it(`a named handle sends a caller's own mobId as given: ${name}`, async () => {
      const run = await drive(name, withOwnMobId(name, "mob-c"), {
        mobId: "mob-b",
        gateway: { features: [MOB_ADDRESSING] },
      });

      const sent = addressed(run);
      assert.notEqual(sent.length, 0);
      for (const { method, params } of sent) {
        assert.equal(params.mob_id, "mob-c", method);
      }
    });
  }

  for (const [title, name, args, expected] of [
    // An event query names the mob whose ledger is read.
    ["an event query with a mob", "queryMobEvents", [{ mobId: "mob-c" }], "mob-c"],
    ["an event query without one", "queryMobEvents", [{ runId: "run-1" }], "mob-b"],
    // A method that sends the caller's own params sends a mob_id among them.
    ["params the caller wrote", "sendDelivery", [{ mob_id: "mob-c" }], "mob-c"],
  ] as const) {
    it(`a named handle keeps a mob the caller named another way: ${title}`, async () => {
      const run = await drive(name, () => [...args], {
        mobId: "mob-b",
        gateway: { features: [MOB_ADDRESSING] },
      });

      assert.deepEqual(
        addressed(run).map((sent) => sent.params.mob_id),
        [expected],
      );
    });
  }

  for (const [title, name, args] of [
    // A method's own mobId, given as null.
    ["a method's own mobId", "announcementGet", [null, "x"]],
    // Params the caller wrote with a null mob_id.
    ["params the caller wrote", "sendDelivery", [{ mob_id: null }]],
    ["an event query", "queryMobEvents", [{ mobId: null }]],
  ] as const) {
    it(`a named handle names its mob where the caller wrote null: ${title}`, async () => {
      // On the wire a null mob_id addresses the launch's own mob, so a named
      // handle never sends one: where the caller wrote null it names its own.
      const run = await drive(name, () => [...args], {
        mobId: "mob-b",
        gateway: { features: [MOB_ADDRESSING] },
      });

      assert.deepEqual(
        addressed(run).map((sent) => ("mob_id" in sent.params ? sent.params.mob_id : "absent")),
        ["mob-b"],
      );
    });
  }

  // Every character a URL reserves, a space and a percent sign, then the
  // four that need no escaping.
  const ID_TO_ESCAPE = "m b/c?d&e=f#g+h!*'()%-_.~";
  const ID_ESCAPED = "m%20b%2Fc%3Fd%26e%3Df%23g%2Bh%21%2A%27%28%29%25-_.~";

  for (const name of Object.keys(STREAM_PATHS).sort()) {
    it(`a stream URL carries the mob id escaped: ${name}`, async () => {
      const run = await drive(name, undefined, {
        mobId: ID_TO_ESCAPE,
        gateway: { features: [MOB_ADDRESSING] },
      });

      // The same bytes from both SDKs: every reserved character percent-encoded.
      assert.deepEqual(run.streams, [`${HTTP_BASE}${STREAM_PATHS[name]}?mob_id=${ID_ESCAPED}`]);
    });
  }

  for (const { id, name, args } of CASES) {
    it(`without the capability a named handle refuses locally: ${id}`, async () => {
      // A gateway that serves the hosting calls and does not honour mob_id.
      const run = await drive(name, args, {
        mobId: "mob-b",
        gateway: { features: [HOSTED_MOBS] },
      });

      if (SENDS_NOTHING.has(name)) {
        assert.deepEqual([requests(run), run.streams], [[], []]);
        return;
      }
      assert.ok(run.error instanceof CapabilityUnavailableError, String(run.error));
      assert.deepEqual(run.error.data, { capability: MOB_ADDRESSING });
      // Nothing but the capability read left: such a gateway would ignore
      // the id and answer for its own mob.
      assert.deepEqual(run.transport, [CAPABILITY_READ]);
      assert.deepEqual([run.multipart, run.streams], [[], []]);
    });
  }

  it("a gateway that lists no feature capabilities is refused too", async () => {
    // An earlier gateway's answer: no `feature_capabilities` member at all.
    const run = await drive("listMembers", undefined, { mobId: "mob-b", gateway: {} });

    assert.ok(run.error instanceof CapabilityUnavailableError, String(run.error));
    assert.deepEqual(run.transport, [CAPABILITY_READ]);
  });

  it("the capability is read once per started transport", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING] });

    const [one, other] = [rt.mob("mob-b"), rt.mob("mob-c")];
    await one.getBlob("blob-1").catch(() => {});
    await one.getBlob("blob-2").catch(() => {});
    await other.getBlob("blob-3").catch(() => {});
    await rt.mobHandle().getBlob("blob-4").catch(() => {});

    assert.deepEqual(run.transport, [
      CAPABILITY_READ,
      { method: "meta/blob/get", params: { blob_id: "blob-1", mob_id: "mob-b" } },
      { method: "meta/blob/get", params: { blob_id: "blob-2", mob_id: "mob-b" } },
      { method: "meta/blob/get", params: { blob_id: "blob-3", mob_id: "mob-c" } },
      { method: "meta/blob/get", params: { blob_id: "blob-4" } },
    ]);

    // A transport started later is another gateway process: it is asked
    // again, and what the first one answered is not carried over.
    const later: Sent[] = [];
    (rt as unknown as Record<string, unknown>)._transport = recordingTransport(later, {
      features: [],
    });
    await assert.rejects(one.getBlob("blob-5"), CapabilityUnavailableError);
    assert.deepEqual(later, [CAPABILITY_READ]);
  });

  it("a transport that is stopped and started again is asked again", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING] });
    const internals = rt as unknown as Record<string, unknown>;
    const transport = internals._transport as Record<string, unknown>;
    transport.stop = async () => {};
    const handle = rt.mob("mob-b");

    await handle.getBlob("blob-1").catch(() => {});
    await rt.shutdown();
    // The same transport object, started again by a later connect.
    internals._transport = transport;
    internals._running = true;
    await handle.getBlob("blob-2").catch(() => {});

    assert.deepEqual(
      run.transport.map((sent) => sent.method),
      ["meta/capabilities", "meta/blob/get", "meta/capabilities", "meta/blob/get"],
    );
  });

  it("an answer is kept for the transport that was asked", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING] });
    const internals = rt as unknown as Record<string, unknown>;
    // Another gateway process takes the first one's place while its
    // capability read is under way; it does not honour mob_id.
    const later: Sent[] = [];
    const asked = internals._transport as Record<string, Callable>;
    const answer = asked.sendAsync;
    asked.sendAsync = async (...sent: unknown[]) => {
      const answered = await answer(...sent);
      if ((sent[0] as Record<string, unknown>).method === "meta/capabilities") {
        internals._transport = recordingTransport(later, { features: [] });
      }
      return answered;
    };
    const handle = rt.mob("mob-b");

    // Where this first request goes is recorded here, not promised: the swap
    // is made inside the capability answer, which the runtime itself never
    // does (it replaces a transport only after that transport's stop).
    await handle.getBlob("blob-1").catch(() => {});
    // What the first process answered is not the second one's answer: the
    // next call asks the transport it is about to use, and is refused.
    await assert.rejects(handle.getBlob("blob-2"), CapabilityUnavailableError);

    assert.deepEqual(run.transport, [CAPABILITY_READ]);
    assert.deepEqual(later.slice(1), [CAPABILITY_READ]);
  });

  it("the default handle never reads the capability", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [] });

    await rt.mobHandle().getBlob("blob-1").catch(() => {});

    assert.deepEqual(run.transport, [{ method: "meta/blob/get", params: { blob_id: "blob-1" } }]);
  });

  const withoutMobId = (sent: readonly Sent[]): Sent[] =>
    sent.map(({ method, params }) => {
      const { mob_id: _named, ...rest } = params;
      return { method, params: rest };
    });

  for (const { id, name, args } of CASES) {
    it(`a named handle sends what the default handle sends, and its id: ${id}`, async () => {
      const gateway = { features: [MOB_ADDRESSING] };
      // A wait sends what is left of its timeout, which it reads from the
      // clock, so two calls made one after the other can differ by a
      // millisecond: both are driven at one instant.
      const clock = Date.now;
      const heldAt = clock();
      Date.now = () => heldAt;
      let byDefault: Run;
      let named: Run;
      try {
        byDefault = await drive(name, args, { gateway });
        named = await drive(name, args, { mobId: "mob-b", gateway });
      } finally {
        Date.now = clock;
      }

      if (SENDS_NOTHING.has(name)) {
        assert.deepEqual([requests(named), requests(byDefault)], [[], []]);
        return;
      }
      // The same requests in the same order, the id aside, after the one read.
      assert.deepEqual(named.transport.slice(0, 1), [CAPABILITY_READ]);
      assert.deepEqual(withoutMobId(named.transport.slice(1)), withoutMobId(byDefault.transport));
      assert.deepEqual(withoutMobId(named.multipart), withoutMobId(byDefault.multipart));
      assert.deepEqual(
        named.streams,
        byDefault.streams.map((url) => `${url}?mob_id=mob-b`),
      );
      // And each request is given the transport timeout the default handle's is.
      assert.deepEqual(named.timeouts.slice(1), byDefault.timeouts);
    });
  }

  it("a wait on a named handle is given its transport timeout", async () => {
    // A wait has the gateway hold its request for the wait's own timeout, so
    // the transport is told to wait longer than its default. A named handle
    // hands that on as the default handle does: without it the wait would
    // end at the transport's default.
    const gateway = { features: [MOB_ADDRESSING] };
    const args = () => [{ target: "startup_ready", timeoutMs: 300_000 }];
    for (const options of [{ gateway }, { mobId: "mob-b", gateway }]) {
      const run = await drive("waitIdentityBootstrap", args, options);
      const waits = run.timeouts.filter(
        (_, at) => run.transport[at].method === "meta/wait_identity_bootstrap",
      );
      assert.deepEqual(waits, [305_000], JSON.stringify(options));
    }
  });

  it("a named handle reaches the wire through a view that holds no state", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING] });
    const heldBefore = Object.getOwnPropertyNames(rt).sort();
    const originalFetch = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response(JSON.stringify({ jsonrpc: "2.0", id: "1", result: {} }))) as typeof fetch;

    const handle = rt.mob("mob-b");
    const view = (handle as unknown as { _runtime: MetaRuntime })._runtime;
    try {
      await handle.getBlob("blob-1").catch(() => {});
      await handle.uploadBlob(png()).catch(() => {});
    } finally {
      globalThis.fetch = originalFetch;
    }

    // The handle of the launch's own mob talks to the runtime itself; a named
    // one talks to a view of it.
    assert.equal((rt.mobHandle() as unknown as { _runtime: MetaRuntime })._runtime, rt);
    assert.notEqual(view, rt);
    assert.ok(view instanceof MetaRuntime);
    // Whatever it sent, the view has its two send paths and nothing else, and
    // can take nothing more; what was learned of the gateway is the runtime's.
    assert.deepEqual(Object.getOwnPropertyNames(view).sort(), ["_rpc", "sseBridge"]);
    assert.equal(Object.isFrozen(view), true);
    assert.deepEqual(Object.getOwnPropertyNames(rt).sort(), heldBefore);
    // It reads the runtime's state as it is now ...
    rt.setRustHttpBase("http://127.0.0.1:10");
    assert.equal(view.rustHttpBaseUrl, "http://127.0.0.1:10");
    // ... and takes no write, by assignment or through a method of the runtime.
    assert.throws(() => {
      (view as unknown as Record<string, unknown>)._running = false;
    }, TypeError);
    assert.throws(() => view.setRustHttpBase("http://127.0.0.1:11"), TypeError);
    assert.equal((rt as unknown as Record<string, unknown>)._running, true);
    assert.equal(rt.rustHttpBaseUrl, "http://127.0.0.1:10");
  });
});

/** The public function members of a class, by name. */
function publicFunctions(owner: { prototype: object }): string[] {
  return Object.getOwnPropertyNames(owner.prototype)
    .filter((name) => name !== "constructor" && !name.startsWith("_"))
    .filter(
      (name) => typeof Object.getOwnPropertyDescriptor(owner.prototype, name)?.value === "function",
    )
    .sort();
}

// Arguments for the calls of the long-term agents a text does not serve.
const IDENTITIES_ARGS: Readonly<Record<string, unknown[]>> = {
  create: [[{ name: "worker-1", profile: "worker" }], { requestId: "request-1" }],
  update: ["worker-1", { expectedSpecDigest: "digest-1", requestId: "request-1", set: {} }],
  archive: ["worker-1", { expectedSpecDigest: "digest-1", requestId: "request-1" }],
  unarchive: ["worker-1", { expectedSpecDigest: "digest-1", requestId: "request-1" }],
  remove: ["worker-1", { expectedSpecDigest: "digest-1", requestId: "request-1" }],
  adopt: ["worker-1", { expectedSpecDigest: "digest-1", requestId: "request-1" }],
  settlePreparation: ["worker-1", { requestId: "request-1" }],
  withdraw: [{ requestId: "request-1" }],
  changesHold: [true],
  operation: [{ requestId: "request-1" }],
  plan: [
    "archive",
    { name: "worker-1", expectedSpecDigest: "digest-1", requestId: "request-1" },
  ],
};
// A member of the class that is not a call of its own.
const IDENTITIES_HELPERS: ReadonlySet<string> = new Set(["change"]);

describe("the runtime's calls by identity, for a handle's mob", () => {
  it("the handle offers each identity-first call of the runtime", () => {
    // Every function member of the runtime is one of three: offered by the
    // handle under its name, offered under the wire's words, or the
    // runtime's alone. A call added to the runtime fails here until it is
    // placed.
    assert.deepEqual(
      publicFunctions(MetaRuntime),
      [
        ...FORWARDED,
        ...Object.values(RENAMED).map(([runtimeName]) => runtimeName),
        ...RUNTIME_ONLY,
      ].sort(),
    );
    // The handle takes each call exactly as the runtime does: the compiler
    // holds the handle's members to the runtime's types.
    const handle = newRuntime(newRun()).mobHandle();
    const sameTypes: Pick<MetaRuntime, (typeof FORWARDED)[number]> = handle;
    const send: MetaRuntime["send"] = handle.sendIdentity;
    const status: MetaRuntime["status"] = handle.statusIdentity;
    for (const name of FORWARDED) assert.equal(typeof sameTypes[name], "function", name);
    assert.deepEqual([typeof send, typeof status], ["function", "function"]);
    assert.ok(handle.identities instanceof IdentitiesHandle);
  });

  for (const [handleName, [runtimeName, runtimeWire, handleWire]] of Object.entries(RENAMED)) {
    it(`a name the handle had keeps its call beside the runtime's: ${handleName}`, async () => {
      for (const mobId of [undefined, "mob-b"]) {
        const gateway = { features: [MOB_ADDRESSING] };
        const own = await drive(runtimeName, undefined, { mobId, gateway });
        const runtimes = await drive(handleName, undefined, { mobId, gateway });
        const first = mobId === undefined ? 0 : 1;
        // `send` is the mob-member send and `sendIdentity` the send by
        // identity, on the default handle and on a named one.
        assert.deepEqual(
          own.transport.slice(first).map((sent) => sent.method),
          [handleWire],
        );
        assert.deepEqual(
          runtimes.transport.slice(first).map((sent) => sent.method),
          [runtimeWire],
        );
        const both = [...own.transport.slice(first), ...runtimes.transport.slice(first)];
        for (const { params } of both) {
          assert.equal(params.mob_id, mobId);
        }
      }
    });
  }

  for (const name of publicFunctions(IdentitiesHandle).filter((n) => !IDENTITIES_HELPERS.has(n))) {
    it(`the long-term agents of a handle name its mob on every call: ${name}`, async () => {
      const sentBy = async (mobId?: string): Promise<Sent[]> => {
        const run = newRun();
        const rt = newRuntime(run, { features: [MOB_ADDRESSING] });
        const handle = mobId === undefined ? rt.mobHandle() : rt.mob(mobId);
        const call = (handle.identities as unknown as Record<string, Callable>)[name];
        const args = IDENTITIES_ARGS[name] ?? ["x"];
        await settle(
          run,
          Promise.resolve().then(() => call.apply(handle.identities, args)),
        );
        return run.transport;
      };
      const byDefault = await sentBy();
      const named = await sentBy("mob-b");

      assert.notEqual(byDefault.length, 0, `IdentitiesHandle.${name} sent nothing`);
      for (const { params } of byDefault) assert.equal("mob_id" in params, false);
      assert.deepEqual(named.slice(0, 1), [CAPABILITY_READ]);
      assert.deepEqual(
        named.slice(1).map((sent) => sent.method),
        byDefault.map((sent) => sent.method),
      );
      for (const { params } of named.slice(1)) assert.equal(params.mob_id, "mob-b");
    });
  }

  it("without the capability the agents of a named handle refuse locally", async () => {
    const run = newRun();
    const handle = newRuntime(run, { features: [] }).mob("mob-b");

    await assert.rejects(handle.sendIdentity("worker-1", "hello"), CapabilityUnavailableError);
    await assert.rejects(handle.identities.list(), CapabilityUnavailableError);

    assert.deepEqual(run.transport, [CAPABILITY_READ]);
  });

  it("the runtime's own calls by identity name no mob", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING] });

    await rt.send("worker-1", "hello").catch(() => {});
    await rt.identities.directory().catch(() => {});

    // No capability read and no mob: the runtime's calls are the launch's mob's.
    assert.deepEqual(run.transport, [
      { method: "meta/send", params: { identity: "worker-1", content: "hello" } },
      { method: "meta/identity/directory", params: {} },
    ]);
  });
});

const MOB_CONFIG = '[mob]\nid = "mob-b"\n';
const DEFAULT_ENTRY = { mob_id: "mob-a", default: true, state: "running", provenance: "host" };
const SECOND_ENTRY = { mob_id: "mob-b", default: false, state: "dormant" };
const MOBS_ANSWERS = {
  "meta/mobs/create": {
    result: { mob: { ...SECOND_ENTRY, state: "running", provenance: "host" } },
  },
  "meta/mobs/list": { result: { mobs: [DEFAULT_ENTRY, SECOND_ENTRY] } },
  "meta/mobs/get": { result: { mob: SECOND_ENTRY } },
};

describe("the mobs of a host", () => {
  it("create, list and get ask the gateway each time", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [HOSTED_MOBS], answers: MOBS_ANSWERS });

    const created = await rt.mobs.create(MOB_CONFIG);
    const listed = await rt.mobs.list();
    await rt.mobs.list();
    const one = await rt.mobs.get("mob-b");
    await rt.mobs.get("mob-b");

    // One capability read, then every call is a request: nothing is remembered.
    assert.deepEqual(run.transport, [
      CAPABILITY_READ,
      { method: "meta/mobs/create", params: { mob_config: MOB_CONFIG } },
      { method: "meta/mobs/list", params: {} },
      { method: "meta/mobs/list", params: {} },
      { method: "meta/mobs/get", params: { mob_id: "mob-b" } },
      { method: "meta/mobs/get", params: { mob_id: "mob-b" } },
    ]);
    assert.deepEqual(
      [created.mobId, created.default, created.state, created.provenance],
      ["mob-b", false, "running", "host"],
    );
    assert.deepEqual(
      listed.map((mob) => [mob.mobId, mob.default, mob.state]),
      [
        ["mob-a", true, "running"],
        ["mob-b", false, "dormant"],
      ],
    );
    // A mob whose creation record was not read has no provenance.
    assert.equal(listed[1].provenance, null);
    assert.deepEqual(one, listed[1]);
  });

  it("create and get take text, or send nothing", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [HOSTED_MOBS], answers: MOBS_ANSWERS });
    const mobs = rt.mobs as unknown as Record<string, (...args: unknown[]) => Promise<unknown>>;

    for (const notText of [undefined, null, 7, { id: "mob-b" }]) {
      await assert.rejects(mobs.create(notText), TypeError);
      await assert.rejects(mobs.get(notText), TypeError);
    }

    assert.deepEqual(run.transport, []);
  });

  it("a hosted mob is the default only when the gateway says true", () => {
    const entry = { mob_id: "mob-b", state: "running" };

    assert.equal(sdk.parseHostedMob({ ...entry, default: true }).default, true);
    // Anything else is not the launch's own mob, whatever its truth value.
    for (const other of [false, null, 1, "true", "yes", [true], { default: true }]) {
      assert.equal(sdk.parseHostedMob({ ...entry, default: other }).default, false, String(other));
    }
    assert.equal(sdk.parseHostedMob(entry).default, false);
  });

  it("an entry carries the revision of a loaded mob", async () => {
    // The revision of a mob's definition: what a redefine is made against.
    // `get` of a loaded mob and the entry of a create, lifecycle or redefine
    // answer carry it; a listed entry and a mob that is not loaded do not.
    const loaded = { ...SECOND_ENTRY, state: "running", provenance: "host", revision: 4 };
    const receipt = {
      mob_id: "mob-b",
      previous_revision: 4,
      committed_revision: 5,
      projection_revision: 9,
      event_cursor: 120,
      declared_fields: [],
    };
    const run = newRun();
    const rt = newRuntime(run, {
      features: [HOSTED_MOBS],
      answers: {
        "meta/mobs/get": { result: { mob: loaded } },
        "meta/mobs/list": { result: { mobs: [DEFAULT_ENTRY, SECOND_ENTRY] } },
        "meta/mobs/redefine": { result: { mob: { ...loaded, revision: 5 }, receipt } },
      },
    });

    const one = await rt.mobs.get("mob-b");
    const listed = await rt.mobs.list();
    const redefined = await rt.mobs.redefine("mob-b", MOB_CONFIG, one.revision as number);

    assert.equal(one.revision, 4);
    assert.deepEqual(
      listed.map((mob) => mob.revision),
      [null, null],
    );
    assert.equal(redefined.mob.revision, 5);
    assert.deepEqual(run.transport.at(-1), {
      method: "meta/mobs/redefine",
      params: { mob_id: "mob-b", mob_config: MOB_CONFIG, expected_revision: 4 },
    });
    // Only a whole number is a revision; anything else reads as none.
    for (const other of [null, "4", 4.5, true, [4]]) {
      assert.equal(
        sdk.parseHostedMob({ ...SECOND_ENTRY, revision: other }).revision,
        null,
        String(other),
      );
    }
    assert.equal(sdk.parseHostedMob({ ...SECOND_ENTRY, revision: 0 }).revision, 0);
  });

  for (const call of ["create", "list", "get"] as const) {
    it(`without the capability the hosting calls refuse locally: ${call}`, async () => {
      const run = newRun();
      // A gateway that honours mob_id for its own mob and hosts no other.
      const rt = newRuntime(run, { features: [MOB_ADDRESSING], answers: MOBS_ANSWERS });
      const refused =
        call === "create"
          ? rt.mobs.create(MOB_CONFIG)
          : call === "list"
            ? rt.mobs.list()
            : rt.mobs.get("mob-b");

      await assert.rejects(refused, (error: unknown) => {
        assert.ok(error instanceof CapabilityUnavailableError);
        assert.deepEqual(error.data, { capability: HOSTED_MOBS });
        assert.equal(error.method, `meta/mobs/${call}`);
        return true;
      });
      assert.deepEqual(run.transport, [CAPABILITY_READ]);
    });
  }

  it("a listed mob's unknown state is kept as text", async () => {
    const run = newRun();
    const rt = newRuntime(run, {
      features: [HOSTED_MOBS],
      answers: {
        "meta/mobs/list": {
          result: {
            mobs: [
              DEFAULT_ENTRY,
              { mob_id: "mob-b", default: false, state: "hibernating", later: 1 },
            ],
          },
        },
      },
    });

    const listed = await rt.mobs.list();

    assert.equal(listed[0].knownState, true);
    assert.deepEqual([listed[1].state, listed[1].knownState], ["hibernating", false]);
  });

  it("a hosted mob reads every state and its optional members", () => {
    assert.deepEqual(
      [...sdk.HOSTED_MOB_STATES].sort(),
      [
        "completed",
        "creating",
        "destroyed",
        "dormant",
        "held",
        "quarantined",
        "running",
        "stopped",
      ],
    );
    for (const state of sdk.HOSTED_MOB_STATES) {
      const entry = sdk.parseHostedMob({ mob_id: "mob-b", default: false, state });
      assert.deepEqual([entry.state, entry.knownState], [state, true]);
      assert.deepEqual([entry.provenance, entry.failure, entry.quarantine], [null, null, null]);
    }

    const quarantine = { class: "store_unreadable", retryable: true, attempts: 2 };
    const quarantined = sdk.parseHostedMob({
      mob_id: "mob-b",
      default: false,
      state: "quarantined",
      quarantine,
    });
    assert.deepEqual(quarantined.quarantine, quarantine);
    const stopped = sdk.parseHostedMob({
      mob_id: "mob-b",
      default: false,
      state: "running",
      provenance: "operator_session",
      failure: { message: "actor stopped" },
    });
    // A creation kind this SDK does not list is kept as text too.
    assert.equal(stopped.provenance, "operator_session");
    assert.deepEqual(stopped.failure, { message: "actor stopped" });
    // An entry without its id or its state is not an entry: an answer this
    // SDK does not read, not a mistake of the caller.
    for (const notAnEntry of [
      { default: false, state: "running" },
      { mob_id: "mob-b", default: false },
      { mob_id: 7, default: false, state: "running" },
      null,
      ["mob-b"],
    ]) {
      assert.throws(() => sdk.parseHostedMob(notAnEntry), ContractMismatchError);
    }
  });

  it("the capability predicates read a result or a list", () => {
    const result = sdk.parseCapabilitiesResult({
      contract_version: "0.9.15",
      methods: [],
      loaded_modules: [],
      feature_capabilities: [MOB_ADDRESSING],
    });
    assert.deepEqual([sdk.MOB_ADDRESSING, sdk.HOSTED_MOBS], [MOB_ADDRESSING, HOSTED_MOBS]);
    assert.equal(sdk.supportsMobAddressing(result), true);
    assert.equal(sdk.supportsHostedMobs(result), false);
    assert.equal(sdk.supportsHostedMobs([HOSTED_MOBS]), true);
    assert.equal(sdk.supportsMobAddressing([]), false);
  });
});

/** What a call rejects with when the gateway answers `error`. */
async function hostingError(data: unknown, code = -32059): Promise<unknown> {
  const error: Record<string, unknown> = { code, message: "refused" };
  if (data !== undefined) error.data = data;
  const rt = newRuntime(newRun(), { answers: { "meta/mobs/create": { error } } });
  try {
    await (rt as unknown as { _rpc: Callable })._rpc("meta/mobs/create", {});
  } catch (caught) {
    return caught;
  }
  throw new Error("the refusal was not raised");
}

describe("the refusal of the hosting calls", () => {
  it("is one error with its kind", async () => {
    assert.equal(sdk.MOB_HOSTING_REFUSED_CODE, -32059);
    assert.deepEqual(
      [...sdk.MOB_HOSTING_REFUSAL_KINDS].sort(),
      ["default_mob", "invalid_mob_id", "mob_busy", "mob_exists", "mob_not_stopped"],
    );
    // The one kind whose request can succeed unchanged later.
    const busy = await hostingError({
      kind: "mob_busy",
      mob_id: "mob-b",
      retryable: true,
      detail: "'mob-b' is stopped and does not close for a new definition; held: owner",
    });
    assert.ok(busy instanceof sdk.MobHostingRefusedError);
    assert.deepEqual([busy.kind, busy.knownKind, busy.retryable], ["mob_busy", true, true]);
    assert.ok(busy.detail.endsWith("held: owner"));
    for (const kind of sdk.MOB_HOSTING_REFUSAL_KINDS) {
      const error = await hostingError({
        kind,
        mob_id: "mob-b",
        retryable: false,
        detail: "why it was refused",
      });
      assert.ok(error instanceof sdk.MobHostingRefusedError);
      assert.ok(error instanceof RpcError);
      assert.deepEqual([error.code, error.method], [-32059, "meta/mobs/create"]);
      assert.deepEqual([error.kind, error.knownKind], [kind, true]);
      assert.equal(sdk.isMobHostingRefusalKind(error.kind), true);
      assert.deepEqual(
        [error.mobId, error.retryable, error.detail],
        ["mob-b", false, "why it was refused"],
      );
    }
    for (const name of [
      "MOB_HOSTING_REFUSED_CODE",
      "MOB_HOSTING_REFUSAL_KINDS",
      "isMobHostingRefusalKind",
      "parseMobHostingRefusal",
      "MobHostingRefusedError",
      "parseHostedMob",
      "HOSTED_MOB_STATES",
      "MobsHandle",
      "MOB_ADDRESSING",
      "HOSTED_MOBS",
      "supportsMobAddressing",
      "supportsHostedMobs",
    ]) {
      assert.ok(name in sdk, name);
    }
  });

  it("a kind this SDK does not list is still the refusal", async () => {
    const error = await hostingError({ kind: "mob_frozen", mob_id: "mob-b", since_ms: 5 });

    // Never a success and never another error: the refusal, with its token.
    assert.ok(error instanceof sdk.MobHostingRefusedError);
    assert.deepEqual([error.kind, error.knownKind], ["mob_frozen", false]);
    assert.equal(error.retryable, null);
    // The gateway sent no sentence: the error's message stands in.
    assert.equal(error.detail, "refused");
    assert.deepEqual(error.details, { since_ms: 5 });
  });

  for (const [title, data] of [
    ["no data", undefined],
    ["text", "text"],
    ["an empty object", {}],
    ["a kind that is not text", { kind: 3 }],
    ["no kind", { mob_id: "mob-b" }],
  ] as const) {
    it(`the code without a kind is a plain RpcError: ${title}`, async () => {
      const error = await hostingError(data);

      assert.ok(error instanceof RpcError);
      assert.equal(Object.getPrototypeOf(error), RpcError.prototype);
      assert.equal(error.code, -32059);
    });
  }

  it("a mob the host does not have stays a plain RpcError", async () => {
    const data = {
      kind: "foreign_mob_target",
      requested_mob_id: "mob-z",
      gateway_mob_id: "mob-a",
    };
    const error = await hostingError(data, -32602);

    assert.ok(error instanceof RpcError);
    assert.equal(Object.getPrototypeOf(error), RpcError.prototype);
    assert.deepEqual(error.data, data);
  });

  it("a refused create raises the hosting refusal", async () => {
    const refusal = {
      code: -32059,
      message: "a mob with the id 'mob-b' exists on this host",
      data: { kind: "mob_exists", mob_id: "mob-b", retryable: false },
    };
    const rt = newRuntime(newRun(), {
      features: [HOSTED_MOBS],
      answers: { "meta/mobs/create": { error: refusal } },
    });

    await assert.rejects(rt.mobs.create(MOB_CONFIG), (error: unknown) => {
      assert.ok(error instanceof sdk.MobHostingRefusedError);
      assert.deepEqual([error.kind, error.mobId], ["mob_exists", "mob-b"]);
      return true;
    });
  });
});

// The requests are held to the wire's own text: `meta/mobs/lifecycle` takes
// `{mob_id, action}`, `meta/mobs/redefine` takes `{mob_id, mob_config,
// expected_revision}` and `meta/mobs/destroy` takes `{mob_id}`, every member
// required.
const CREATED_ENTRY = { ...SECOND_ENTRY, state: "running", provenance: "host" };
const STOPPED_ENTRY = { mob_id: "mob-b", default: false, state: "stopped", provenance: "host" };
const STOP_REPORT = { members_stopped: 2, inflight: [] };
const RECEIPT = {
  mob_id: "mob-b",
  previous_revision: 4,
  committed_revision: 5,
  projection_revision: 9,
  event_cursor: 120,
  declared_fields: ["profiles.worker.model"],
};
const DESTROY_REPORT = { storage_removed: true };
const CHANGE_ANSWERS = {
  "meta/mobs/lifecycle": { result: { mob: STOPPED_ENTRY, stop_report: STOP_REPORT } },
  "meta/mobs/redefine": { result: { mob: CREATED_ENTRY, receipt: RECEIPT } },
  "meta/mobs/destroy": { result: { mob_id: "mob-b", destroy_report: DESTROY_REPORT } },
};

describe("stop, resume, redefine and destroy a mob of the host", () => {
  it("lifecycle, redefine and destroy send the wire's requests", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [HOSTED_MOBS], answers: CHANGE_ANSWERS });

    const stopped = await rt.mobs.lifecycle("mob-b", "stop");
    await rt.mobs.lifecycle("mob-b", "resume");
    const redefined = await rt.mobs.redefine("mob-b", MOB_CONFIG, 4);
    const destroyed = await rt.mobs.destroy("mob-b");

    // Method and params as JSON text, members in the order sent.
    assert.deepEqual(
      run.transport.map((sent) => [sent.method, JSON.stringify(sent.params)]),
      [
        ["meta/capabilities", "{}"],
        ["meta/mobs/lifecycle", '{"mob_id":"mob-b","action":"stop"}'],
        ["meta/mobs/lifecycle", '{"mob_id":"mob-b","action":"resume"}'],
        [
          "meta/mobs/redefine",
          JSON.stringify({ mob_id: "mob-b", mob_config: MOB_CONFIG, expected_revision: 4 }),
        ],
        ["meta/mobs/destroy", '{"mob_id":"mob-b"}'],
      ],
    );
    // Each answer is typed; what Core reports is handed on as it came.
    assert.deepEqual([stopped.mob.mobId, stopped.mob.state], ["mob-b", "stopped"]);
    assert.deepEqual(stopped.stopReport, STOP_REPORT);
    assert.deepEqual(redefined.mob, sdk.parseHostedMob(CREATED_ENTRY));
    assert.deepEqual(redefined.receipt, {
      mobId: "mob-b",
      previousRevision: 4,
      committedRevision: 5,
      projectionRevision: 9,
      eventCursor: 120,
      declaredFields: ["profiles.worker.model"],
    });
    assert.deepEqual([destroyed.mobId, destroyed.destroyReport], ["mob-b", DESTROY_REPORT]);
    for (const name of [
      "parseMobLifecycleResult",
      "parseMobRedefineResult",
      "parseMobDestroyResult",
    ]) {
      assert.ok(name in sdk, name);
    }
  });

  it("a resume answers no stop report", async () => {
    const rt = newRuntime(newRun(), {
      features: [HOSTED_MOBS],
      answers: { "meta/mobs/lifecycle": { result: { mob: CREATED_ENTRY } } },
    });

    const resumed = await rt.mobs.lifecycle("mob-b", "resume");

    assert.deepEqual([resumed.mob.state, resumed.stopReport], ["running", null]);
  });

  for (const call of ["lifecycle", "redefine", "destroy"] as const) {
    it(`without the capability the changes of a hosted mob refuse locally: ${call}`, async () => {
      const run = newRun();
      const rt = newRuntime(run, { features: [MOB_ADDRESSING], answers: CHANGE_ANSWERS });
      const refused =
        call === "lifecycle"
          ? rt.mobs.lifecycle("mob-b", "stop")
          : call === "redefine"
            ? rt.mobs.redefine("mob-b", MOB_CONFIG, 4)
            : rt.mobs.destroy("mob-b");

      await assert.rejects(refused, (error: unknown) => {
        assert.ok(error instanceof CapabilityUnavailableError);
        assert.deepEqual(error.data, { capability: HOSTED_MOBS });
        assert.equal(error.method, `meta/mobs/${call}`);
        return true;
      });
      assert.deepEqual(run.transport, [CAPABILITY_READ]);
    });
  }

  it("a change of a hosted mob names its mob and its input, or sends nothing", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [HOSTED_MOBS], answers: CHANGE_ANSWERS });
    const mobs = rt.mobs as unknown as Record<string, (...args: unknown[]) => Promise<unknown>>;

    // The wire requires every member: a call that lacks one is refused here.
    await assert.rejects(mobs.lifecycle("mob-b", "pause"), TypeError);
    for (const notAnId of [undefined, null, 7]) {
      await assert.rejects(mobs.lifecycle(notAnId, "stop"), TypeError);
      await assert.rejects(mobs.redefine(notAnId, MOB_CONFIG, 4), TypeError);
      await assert.rejects(mobs.destroy(notAnId), TypeError);
    }
    await assert.rejects(mobs.redefine("mob-b", undefined, 4), TypeError);
    for (const notARevision of [undefined, null, "4", 4.5, -1, Number.NaN]) {
      await assert.rejects(mobs.redefine("mob-b", MOB_CONFIG, notARevision), TypeError);
    }

    assert.deepEqual(run.transport, []);
  });

  it("a destroy of the launch's own mob raises the hosting refusal", async () => {
    const refusal = {
      code: -32059,
      message: "the mob 'mob-a' is the mob this launch names",
      data: { kind: "default_mob", mob_id: "mob-a", retryable: false },
    };
    const rt = newRuntime(newRun(), {
      features: [HOSTED_MOBS],
      answers: { "meta/mobs/destroy": { error: refusal } },
    });

    await assert.rejects(rt.mobs.destroy("mob-a"), (error: unknown) => {
      assert.ok(error instanceof sdk.MobHostingRefusedError);
      assert.deepEqual([error.kind, error.knownKind, error.mobId], ["default_mob", true, "mob-a"]);
      return true;
    });
  });

  for (const [title, call, error] of [
    // A mob the host does not have, on every call that names one.
    [
      "a mob the host does not have",
      "lifecycle",
      {
        code: -32602,
        message: "request targets mob 'mob-z' but this gateway serves 'mob-a'",
        data: { kind: "foreign_mob_target", requested_mob_id: "mob-z", gateway_mob_id: "mob-a" },
      },
    ],
    // A mob whose store did not load keeps the answer it has elsewhere.
    [
      "a quarantined mob",
      "destroy",
      {
        code: -32603,
        message: "mob 'mob-b' is quarantined",
        data: { kind: "mob_quarantined", mob_id: "mob-b", retryable: true },
      },
    ],
    // A read whose load of the mob failed answers what a change of that mob
    // would answer.
    [
      "a read of a mob whose load failed",
      "get",
      {
        code: -32603,
        message: "mob 'mob-b' is quarantined",
        data: { kind: "mob_quarantined", mob_id: "mob-b", retryable: true },
      },
    ],
  ] as const) {
    it(`answers that are not the hosting refusal stay plain RpcErrors: ${title}`, async () => {
      const rt = newRuntime(newRun(), {
        features: [HOSTED_MOBS],
        answers: { [`meta/mobs/${call}`]: { error } },
      });
      const raised =
        call === "lifecycle"
          ? rt.mobs.lifecycle("mob-z", "stop")
          : call === "get"
            ? rt.mobs.get("mob-b")
            : rt.mobs.destroy("mob-b");

      await assert.rejects(raised, (caught: unknown) => {
        assert.ok(caught instanceof RpcError);
        assert.equal(Object.getPrototypeOf(caught), RpcError.prototype);
        assert.equal(caught.code, error.code);
        assert.deepEqual(caught.data, "data" in error ? error.data : undefined);
        return true;
      });
    });
  }

  it("a storage refusal without data keeps its error class", async () => {
    // Pin the SDK's existing code translation, not when a gateway sends it.
    const error = { code: -32014, message: "the storage cannot serve this call" };
    const run = newRun();
    const rt = newRuntime(run, {
      features: [HOSTED_MOBS],
      answers: { "meta/mobs/get": { error } },
    });

    await assert.rejects(rt.mobs.get("mob-b"), (caught: unknown) => {
      assert.ok(caught instanceof StorageResolutionError);
      assert.equal(Object.getPrototypeOf(caught), StorageResolutionError.prototype);
      assert.equal(caught.code, error.code);
      assert.equal(caught.message, error.message);
      assert.equal(caught.data, undefined);
      return true;
    });
    assert.deepEqual(run.transport, [
      { method: "meta/capabilities", params: {} },
      { method: "meta/mobs/get", params: { mob_id: "mob-b" } },
    ]);
  });

  it("an answer without its parts is not read as a result", () => {
    for (const malformed of [null, {}, { mob: { default: false } }]) {
      assert.throws(() => sdk.parseMobLifecycleResult(malformed), ContractMismatchError);
    }
    const { event_cursor: _cursor, ...withoutARevision } = RECEIPT;
    for (const receipt of [
      undefined,
      { mob_id: "mob-b" },
      // Every member of the receipt is required, each of its kind.
      withoutARevision,
      { ...RECEIPT, committed_revision: "5" },
      { ...RECEIPT, previous_revision: 4.5 },
      { ...RECEIPT, declared_fields: "profiles.worker.model" },
      { ...RECEIPT, mob_id: null },
    ]) {
      const malformed =
        receipt === undefined ? { mob: CREATED_ENTRY } : { mob: CREATED_ENTRY, receipt };
      assert.throws(
        () => sdk.parseMobRedefineResult(malformed),
        ContractMismatchError,
        JSON.stringify(receipt),
      );
    }
    for (const malformed of [null, {}, { mob_id: 7 }]) {
      assert.throws(() => sdk.parseMobDestroyResult(malformed), ContractMismatchError);
    }
    // A report Core did not send is none, not an error.
    assert.equal(sdk.parseMobDestroyResult({ mob_id: "mob-b" }).destroyReport, null);
    assert.equal(
      sdk.parseMobDestroyResult({ mob_id: "mob-b", destroy_report: null }).destroyReport,
      null,
    );
  });

  for (const [title, method, answer, call] of [
    ["create, no entry", "meta/mobs/create", {}, (mobs) => mobs.create(MOB_CONFIG)],
    ["create, no object", "meta/mobs/create", "created", (mobs) => mobs.create(MOB_CONFIG)],
    ["list, no list", "meta/mobs/list", {}, (mobs) => mobs.list()],
    ["list, a text", "meta/mobs/list", { mobs: "mob-a" }, (mobs) => mobs.list()],
    [
      "list, an entry without its state",
      "meta/mobs/list",
      { mobs: [{ mob_id: "mob-a" }] },
      (mobs) => mobs.list(),
    ],
    ["get, no entry", "meta/mobs/get", { mob: null }, (mobs) => mobs.get("mob-b")],
    ["lifecycle, no entry", "meta/mobs/lifecycle", {}, (mobs) => mobs.lifecycle("mob-b", "resume")],
    [
      "redefine, no receipt",
      "meta/mobs/redefine",
      { mob: CREATED_ENTRY },
      (mobs) => mobs.redefine("mob-b", MOB_CONFIG, 4),
    ],
    ["destroy, no id", "meta/mobs/destroy", {}, (mobs) => mobs.destroy("mob-b")],
    [
      "destroy, a report without the id",
      "meta/mobs/destroy",
      { destroy_report: DESTROY_REPORT },
      (mobs) => mobs.destroy("mob-b"),
    ],
  ] as ReadonlyArray<
    readonly [string, string, unknown, (mobs: MetaRuntime["mobs"]) => Promise<unknown>]
  >) {
    it(`an answer the SDK cannot read is not the caller's mistake: ${title}`, async () => {
      // TypeError is for the caller's arguments, refused before anything is
      // sent. Here the request left and the gateway acted on it: the caller
      // must be able to tell the two apart, most of all for a destroy, so an
      // unreadable answer has a class of its own.
      const run = newRun();
      const rt = newRuntime(run, {
        features: [HOSTED_MOBS],
        answers: { [method]: { result: answer } },
      });

      await assert.rejects(call(rt.mobs), (error: unknown) => {
        assert.ok(error instanceof ContractMismatchError, String(error));
        assert.ok(!(error instanceof TypeError));
        return true;
      });

      assert.deepEqual(
        run.transport.map((sent) => sent.method),
        ["meta/capabilities", method],
      );
    });
  }

  it("a stop without its report is not read as a stop", async () => {
    // The wire answers a stop with the mob's stop report. An answer without
    // one is no answer to a stop: it is refused, typed, not read as "none".
    for (const answer of [{ mob: STOPPED_ENTRY }, { mob: STOPPED_ENTRY, stop_report: null }]) {
      const rt = newRuntime(newRun(), {
        features: [HOSTED_MOBS],
        answers: { "meta/mobs/lifecycle": { result: answer } },
      });
      await assert.rejects(rt.mobs.lifecycle("mob-b", "stop"), ContractMismatchError);
      // A resume has no report to bring.
      assert.equal((await rt.mobs.lifecycle("mob-b", "resume")).stopReport, null);
    }
  });

  for (const report of ["stopped", 7, true, ["members_stopped"]]) {
    it(`a report that is not an object is not read as none: ${JSON.stringify(report)}`, () => {
      assert.throws(
        () => sdk.parseMobLifecycleResult({ mob: STOPPED_ENTRY, stop_report: report }),
        ContractMismatchError,
      );
      assert.throws(
        () => sdk.parseMobDestroyResult({ mob_id: "mob-b", destroy_report: report }),
        ContractMismatchError,
      );
    });
  }
});

// The handle of a named mob is the one way to name a mob on these calls: none
// of them takes a `mobId` of its own. The same agent name under two handles
// is two agents, each with its own memory.

// The five memory calls: what each is given here, its wire method, and the
// params that leave besides the mob.
const MEMORY_CALLS: Readonly<Record<string, readonly [unknown[], string, Record<string, unknown>]>> = {
  rememberAgentMemory: [
    ["scribe", { title: "t", body: "b", tags: ["x"] }],
    "meta/agent_memory/remember",
    { identity: "scribe", title: "t", body: "b", tags: ["x"] },
  ],
  recallAgentMemory: [
    ["scribe", { selection: "always" }],
    "meta/agent_memory/recall",
    { identity: "scribe", selection: "always" },
  ],
  updateAgentMemory: [
    ["scribe", "m-1", { title: "t2", body: "b2" }],
    "meta/agent_memory/update",
    { identity: "scribe", memory_id: "m-1", title: "t2", body: "b2" },
  ],
  forgetAgentMemory: [
    ["scribe", "m-1"],
    "meta/agent_memory/forget",
    { identity: "scribe", memory_id: "m-1" },
  ],
  manifestAgentMemory: [
    ["scribe", { tier: "full" }],
    "meta/agent_memory/manifest",
    { identity: "scribe", tier: "full" },
  ],
};
const MEMORY_RECORD = {
  memory_id: "m-1",
  title: "t",
  body: "b",
  tags: [],
  created_at_ms: 1,
  updated_at_ms: 1,
};
// What a host answers for a mob that has no long-term agents of its own yet.
const NO_IDENTITY_CONTEXT = {
  kind: "foreign_mob_target",
  requested_mob_id: "mob-b",
  gateway_mob_id: "mob-a",
  hosted: true,
  identity_context: false,
};

describe("an agent's memory and the long-term agents, mob by mob", () => {
  it("the memory calls are the handle's five", () => {
    assert.deepEqual(
      NAMES.filter((name) => name.endsWith("AgentMemory")),
      Object.keys(MEMORY_CALLS).sort(),
    );
  });

  for (const [name, [given, method, params]] of Object.entries(MEMORY_CALLS)) {
    it(`a memory call of a named handle names its mob and no realm: ${name}`, async () => {
      const gateway = { features: [MOB_ADDRESSING] };
      const byDefault = await drive(name, () => given, { gateway });
      const named = await drive(name, () => given, { mobId: "mob-b", gateway });

      // The launch's own mob: the call's params and nothing else, no capability read.
      assert.deepEqual(byDefault.transport, [{ method, params }]);
      // A named mob: the same params and the mob, after the one capability
      // read. No realm leaves unless the caller gave one: the gateway fills
      // the mob's own.
      assert.deepEqual(addressed(named), [{ method, params: { ...params, mob_id: "mob-b" } }]);
    });

    it(`a mob in a memory call's options is not sent: ${name}`, async () => {
      // The options name no mob: one written there by a caller that bypasses
      // the types leaves nothing on the wire, on either handle.
      const last = given[given.length - 1];
      const withMob =
        typeof last === "object" && last !== null
          ? [...given.slice(0, -1), { ...last, mobId: "mob-c", mob_id: "mob-c" }]
          : [...given, { mobId: "mob-c", mob_id: "mob-c" }];
      const gateway = { features: [MOB_ADDRESSING] };
      const byDefault = await drive(name, () => withMob, { gateway });
      const named = await drive(name, () => withMob, { mobId: "mob-b", gateway });

      assert.deepEqual(byDefault.transport, [{ method, params }]);
      assert.deepEqual(addressed(named), [{ method, params: { ...params, mob_id: "mob-b" } }]);
    });

    for (const [title, gateway] of [
      ["no feature capabilities", {}],
      ["an empty list", { features: [] }],
      ["another capability", { features: [HOSTED_MOBS] }],
    ] as const) {
      it(`without the capability a memory call of a named handle sends nothing: ${name}, ${title}`, async () => {
        // A gateway that does not honour mob_id would act for the agent of
        // that name in its own mob: the call is refused here and never leaves.
        const run = await drive(name, () => given, { mobId: "mob-b", gateway });

        assert.ok(run.error instanceof CapabilityUnavailableError, String(run.error));
        assert.deepEqual(run.error.data, { capability: MOB_ADDRESSING });
        assert.deepEqual(run.transport, [CAPABILITY_READ]);
        assert.deepEqual([run.multipart, run.streams], [[], []]);
      });
    }
  }

  it("one agent name under two mobs is two memories", async () => {
    const run = newRun();
    const rt = newRuntime(run, {
      features: [MOB_ADDRESSING],
      answers: {
        "meta/agent_memory/remember": { result: MEMORY_RECORD },
        "meta/agent_memory/recall": { result: { records: [MEMORY_RECORD] } },
        "meta/agent_memory/forget": { result: { memory_id: "m-1", deleted: true } },
      },
    });
    const first = rt.mob("mob-a");
    const second = rt.mob("mob-b");

    const kept = await first.rememberAgentMemory("scribe", { title: "t", body: "b" });
    await second.rememberAgentMemory("scribe", { title: "t", body: "b" });
    const recalled = await second.recallAgentMemory("scribe", { selection: "always" });
    await first.forgetAgentMemory("scribe", kept.memoryId);

    assert.deepEqual(
      recalled.map((record) => record.memoryId),
      ["m-1"],
    );
    // One capability read for both handles, then each request under its
    // handle's mob: the name is the same text, the mob is what tells the two
    // agents apart.
    const remember = { identity: "scribe", title: "t", body: "b" };
    assert.deepEqual(run.transport, [
      CAPABILITY_READ,
      { method: "meta/agent_memory/remember", params: { ...remember, mob_id: "mob-a" } },
      { method: "meta/agent_memory/remember", params: { ...remember, mob_id: "mob-b" } },
      {
        method: "meta/agent_memory/recall",
        params: { identity: "scribe", selection: "always", mob_id: "mob-b" },
      },
      {
        method: "meta/agent_memory/forget",
        params: { identity: "scribe", memory_id: "m-1", mob_id: "mob-a" },
      },
    ]);
  });

  for (const [title, error] of [
    [
      "no identity context",
      { code: -32602, message: "Invalid params", data: NO_IDENTITY_CONTEXT },
    ],
    ["not configured", { code: -32601, message: "agent memory is not configured" }],
    ["not configured, as params", { code: -32602, message: "agent memory is not configured" }],
  ] as const) {
    it(`a mob that keeps no memory answers a plain RpcError: ${title}`, async () => {
      const rt = newRuntime(newRun(), {
        features: [MOB_ADDRESSING],
        answers: { "meta/agent_memory/recall": { error } },
      });

      // No new error and no new kind: the answer as the gateway gave it.
      await assert.rejects(rt.mob("mob-b").recallAgentMemory("scribe"), (caught: unknown) => {
        assert.ok(caught instanceof RpcError);
        assert.equal(Object.getPrototypeOf(caught), RpcError.prototype);
        assert.equal(caught.code, error.code);
        assert.deepEqual(caught.data, "data" in error ? error.data : undefined);
        return true;
      });
    });
  }

  it("one agent name in two mobs is two agents", async () => {
    const agent = {
      name: "scribe",
      profile: "writer",
      spec_digest: `sha256:${"a".repeat(64)}`,
      archived: false,
      readiness: "dormant",
    };
    const run = newRun();
    const rt = newRuntime(run, {
      features: [MOB_ADDRESSING],
      answers: {
        "meta/identity/create": {
          result: {
            outcome: "done",
            operation_id: "op-1",
            agents: [{ name: "scribe", state: "committed" }],
          },
        },
        "meta/identity/list": { result: { agents: [agent] } },
      },
    });

    for (const handle of [rt.mobHandle(), rt.mob("mob-b")]) {
      const created = await handle.identities.create([{ name: "scribe", profile: "writer" }], {
        requestId: "create-scribe",
      });
      assert.equal(created.outcome, "done");
      const page = await handle.identities.list();
      assert.deepEqual(
        page.agents.map((row) => row.name),
        ["scribe"],
      );
      await handle.sendIdentity("scribe", "hello").catch(() => {});
    }

    // The same calls with the same request id, once for the launch's own mob
    // and once for the named one: the mob is the only difference, and the
    // caller's request id leaves as written.
    const create = {
      request_id: "create-scribe",
      agents: [{ name: "scribe", profile: "writer" }],
    };
    const send = { identity: "scribe", content: "hello" };
    assert.deepEqual(run.transport, [
      { method: "meta/identity/create", params: create },
      { method: "meta/identity/list", params: { include_archived: false } },
      { method: "meta/send", params: send },
      CAPABILITY_READ,
      { method: "meta/identity/create", params: { ...create, mob_id: "mob-b" } },
      { method: "meta/identity/list", params: { include_archived: false, mob_id: "mob-b" } },
      { method: "meta/send", params: { ...send, mob_id: "mob-b" } },
    ]);
  });

  for (const [title, data] of [
    // The host has the mob and does not serve the long-term agents' calls for it.
    [
      "not served",
      Object.fromEntries(
        Object.entries(NO_IDENTITY_CONTEXT).filter(([key]) => key !== "identity_context"),
      ),
    ],
    // The host has the mob and the mob has no identity context.
    ["no identity context", NO_IDENTITY_CONTEXT],
  ] as const) {
    it(`a mob without long-term agents of its own answers a plain RpcError: ${title}`, async () => {
      const error = { code: -32602, message: "Invalid params", data };
      const run = newRun();
      const rt = newRuntime(run, {
        features: [MOB_ADDRESSING],
        answers: { "meta/identity/list": { error } },
      });

      // Not a refusal of the family and not one of the hosting calls: the
      // data says the host has the mob, and is kept whole for the caller.
      await assert.rejects(rt.mob("mob-b").identities.list(), (caught: unknown) => {
        assert.ok(caught instanceof RpcError);
        assert.equal(Object.getPrototypeOf(caught), RpcError.prototype);
        assert.deepEqual([caught.code, caught.data], [-32602, data]);
        return true;
      });
      // The request left for the named mob: the answer is the gateway's.
      assert.deepEqual(run.transport[1], {
        method: "meta/identity/list",
        params: { include_archived: false, mob_id: "mob-b" },
      });
    });
  }

  it("a refusal of the family for a named mob is the family's error", async () => {
    const error = { code: -32058, message: "refused: name_taken", data: { kind: "name_taken" } };
    const run = newRun();
    const rt = newRuntime(run, {
      features: [MOB_ADDRESSING],
      answers: { "meta/identity/create": { error } },
    });

    await assert.rejects(
      rt.mob("mob-b").identities.create([{ name: "scribe", profile: "writer" }], {
        requestId: "create-scribe",
      }),
      (caught: unknown) => {
        assert.ok(caught instanceof sdk.IdentityRefusedError);
        assert.equal(caught.kind, "name_taken");
        return true;
      },
    );
    assert.equal(run.transport[1].params.mob_id, "mob-b");
  });
});

describe("the mob of an event query", () => {
  it("sends the mob it is given, and the handle's otherwise", async () => {
    const query = { memberId: "worker-1", afterSeq: 7, limit: 5 };
    const wire = { member_id: "worker-1", after_seq: 7, limit: 5 };
    const run = newRun();
    const rt = newRuntime(run, { features: [MOB_ADDRESSING] });

    // The handle of the launch's own mob: the caller's mob, or none.
    await rt.mobHandle().queryEvents({ ...query, mobId: "mob-c" });
    await rt.mobHandle().queryEvents(query);
    // A named handle: the caller's mob when one is given, its own otherwise.
    await rt.mob("mob-b").queryEvents({ ...query, mobId: "mob-c" });
    await rt.mob("mob-b").queryEvents(query);

    // A query that names a mob asks the gateway first, once, whichever
    // handle sends it; a query of the launch's own mob that names none asks
    // nothing.
    assert.deepEqual(run.transport, [
      CAPABILITY_READ,
      { method: "meta/query_events", params: { ...wire, mob_id: "mob-c" } },
      { method: "meta/query_events", params: wire },
      { method: "meta/query_events", params: { ...wire, mob_id: "mob-c" } },
      { method: "meta/query_events", params: { ...wire, mob_id: "mob-b" } },
    ]);
  });

  for (const [title, gateway] of [
    ["no feature capabilities", {}],
    ["an empty list", { features: [] }],
    ["another capability", { features: [HOSTED_MOBS] }],
  ] as const) {
    it(`without the capability the launch's handle sends no query that names a mob: ${title}`, async () => {
      // A gateway that does not honour mob_id would answer such a query
      // without an error and for its own mob: it is refused here instead.
      const run = newRun();
      const handle = newRuntime(run, gateway).mobHandle();

      await assert.rejects(
        handle.queryEvents({ memberId: "worker-1", mobId: "mob-c" }),
        (caught: unknown) => {
          assert.ok(caught instanceof CapabilityUnavailableError, String(caught));
          assert.deepEqual(caught.data, { capability: MOB_ADDRESSING });
          return true;
        },
      );
      assert.deepEqual(run.transport, [CAPABILITY_READ]);

      // A query that names no mob is the launch's own mob's, as before.
      assert.deepEqual(await handle.queryEvents({ memberId: "worker-1" }), []);
      assert.deepEqual(run.transport.slice(1), [
        { method: "meta/query_events", params: { member_id: "worker-1" } },
      ]);
    });
  }

  it("a query of the launch's handle that names no mob reads no capability", async () => {
    const run = newRun();
    const rt = newRuntime(run, { features: [] });

    await rt.mobHandle().queryEvents({ memberId: "worker-1" });

    assert.deepEqual(run.transport, [
      { method: "meta/query_events", params: { member_id: "worker-1" } },
    ]);
  });
});
