/**
 * Typed error hierarchy for the Meta SDK.
 *
 * @example
 * ```ts
 * import { RpcError, MetaError } from "@rkat/meta-sdk";
 *
 * try {
 *   await handle.status();
 * } catch (err) {
 *   if (err instanceof RpcError) {
 *     console.error(`RPC ${err.method} failed: code=${err.code}`);
 *   }
 * }
 * ```
 */

import type { TransportTermination } from "./transport.js";

// -- Base error -----------------------------------------------------------

/** Base exception for all Meta SDK errors. */
export class MetaError extends Error {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "MetaError";
  }
}

// -- Transport errors -----------------------------------------------------

/** Raised when the transport layer fails (subprocess died, connection refused, etc.). */
export class TransportError extends MetaError {
  constructor(message: string, options?: ErrorOptions) {
    super(message, options);
    this.name = "TransportError";
  }
}

/**
 * The SDK stopped waiting for `meta/init` before it settled.
 *
 * Thrown when the init request was written but no settlement arrived: the
 * acceptance was lost, the reader failed, the gateway exited, or the
 * caller's init deadline ran out. The gateway may have changed native state
 * (owner publication, identities, schedules) before it stopped, so this is
 * never reported as a refusal and never retried automatically. `initId`
 * correlates with the gateway's progress notifications; `lastPhase` is the
 * last `meta/init_progress` phase seen (`null` when none arrived); `reason`
 * names what ended the wait.
 */
export class InitOutcomeUnknownError extends MetaError {
  constructor(
    readonly initId: string,
    readonly lastPhase: string | null,
    readonly reason: string,
  ) {
    super(
      `meta/init outcome unknown (init_id=${initId}, last phase: ${lastPhase ?? "none"}): ` +
        `${reason}; native state may have changed`,
    );
    this.name = "InitOutcomeUnknownError";
  }
}

/**
 * The transport's reader stopped, so no response can arrive any more.
 *
 * Rejects every request still waiting when the gateway closes its stdout,
 * and every later request on the same gateway process. `reason` names what
 * ended the reader. A request written before the reader stopped may still
 * have been executed by the gateway.
 */
export class TransportReaderFailedError extends TransportError {
  constructor(readonly reason: string) {
    super(`transport reader stopped: ${reason}`);
    this.name = "TransportReaderFailedError";
  }
}

/**
 * The gateway process exited while a process it started still held its
 * stdout, so the reader never saw that stdout end.
 *
 * Rejects every request still waiting once a short grace after the exit has
 * passed, instead of leaving it to its request timeout. `exitCode` and
 * `signal` are the exit as Node reported it. As with any reader failure, a
 * request written before the exit may still have been executed.
 */
export class GatewayChildEndedError extends TransportReaderFailedError {
  constructor(
    reason: string,
    readonly exitCode: number | null,
    readonly signal: string | null,
  ) {
    super(reason);
    this.name = "GatewayChildEndedError";
  }
}

/**
 * A host-composed gateway transport refused to respawn its gateway process.
 *
 * A transport built by a gateway transport factory, or one with its own
 * `startAsync()`, finishes a trusted host startup that a plain respawn would
 * skip. Once its process is gone, requests fail with this error until the
 * runtime reconnects, which builds and starts the transport again.
 */
export class GatewayRestartRefusedError extends TransportError {
  constructor(readonly reason: string) {
    super(`host-composed gateway transport will not respawn: ${reason}; reconnect the runtime`);
    this.name = "GatewayRestartRefusedError";
  }
}

/**
 * The cleanup of a runtime's gateway transport, reached through that same
 * runtime: `shutdown()` retries its retained stop (bounded per call) and
 * `lastTermination()` reads what the last stop observed.
 */
export interface GatewayCleanup {
  shutdown(): Promise<void>;
  lastTermination(): TransportTermination | null;
}

/**
 * A runtime's startup failed and the stop of the gateway transport it made
 * then rejected, so the runtime keeps that transport until a stop fulfils.
 * The rejection is not proof that the gateway process still runs: a process
 * can exit while its transport's own cleanup fails, and
 * `cleanup.lastTermination()` tells what the stop observed.
 *
 * `startupError` (also the `cause`) is the startup failure exactly as it is
 * thrown when the stop fulfils; `cleanupError` is the stop's rejection,
 * exactly as thrown. `cleanup` reaches the same runtime even when
 * `MetaBuilder.build()` returned none; its `shutdown()` retries that stop.
 * A startup failure whose stop fulfilled throws its original error unchanged.
 */
export class GatewayStartupCleanupError extends TransportError {
  constructor(
    readonly startupError: unknown,
    readonly cleanupError: unknown,
    readonly cleanup: GatewayCleanup,
  ) {
    // A fixed message: formatting either failure can itself throw (an object
    // without a prototype, a throwing `message` getter) and would lose the
    // cleanup this error carries.
    super(
      "gateway startup failed and the stop of its gateway transport was rejected " +
        "(see startupError and cleanupError)",
      { cause: startupError },
    );
    this.name = "GatewayStartupCleanupError";
  }
}

// -- RPC errors -----------------------------------------------------------

/** Raised when a JSON-RPC call returns an error response. */
export class RpcError extends MetaError {
  constructor(
    readonly code: number,
    message: string,
    readonly requestId: string,
    readonly method: string,
    /** Optional structured payload from the JSON-RPC `error.data` field. */
    readonly data?: unknown,
  ) {
    super(message);
    this.name = "RpcError";
  }
}

/**
 * JSON-RPC error code returned when the caller's `after_seq` is past the
 * current ledger frontier. Returned by `meta/mob_events/{query,subscribe}`
 * and the `/meta/mob_events/stream` SSE route (HTTP 410 Gone).
 */
export const MOB_EVENTS_STALE_CURSOR_CODE = -32010 as const;
export const CAPABILITY_UNAVAILABLE_CODE = -32004 as const;
/**
 * A send named the registered row its input was authored from
 * (`expectedSpecDigest`) and the identity's registered row is no longer that
 * row. Checked before delivery: nothing was submitted.
 */
export const REGISTERED_SPEC_CHANGED_CODE = -32006 as const;
export const MEMORY_BACKEND_UNAVAILABLE_CODE = -32012 as const;
export const CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE = -32013 as const;
/**
 * Fail-closed storage refusal at gateway startup (`meta/init`): file-name
 * twins the storage layout refuses to pick between, a store that failed to
 * open where the silent fallback used to be, or a state-root creation
 * failure. The message carries the remediation (the storage doctor, or the
 * explicit ephemeral declaration).
 */
export const STORAGE_RESOLUTION_CODE = -32014 as const;
/**
 * An ordinary request sent before `meta/init` settled: the stdio gateway
 * refuses it at once instead of queueing it behind startup (a provider
 * callback awaiting it during init would wait on itself).
 */
export const INIT_IN_PROGRESS_CODE = -32018 as const;
/**
 * Mob composition provenance refusal at gateway startup (`meta/init`): the
 * persistent mob storage's recorded composition cannot be proven to match
 * the launch. The error data carries the refusal's `kind` and, where it has
 * them, the diverged `fields`.
 */
export const COMPOSITION_PROVENANCE_CODE = -32019 as const;
/**
 * meerkat's typed refusal for a member whose runtime registration must be
 * reloaded before it accepts work (`meta/force_cancel_member`). The error data
 * carries `kind: MEMBER_RELOAD_REQUIRED_KIND` and names the member and the
 * repair, `required_action: "reload_member_registration"`.
 */
export const MEMBER_RELOAD_REQUIRED_CODE = -32029 as const;
/** The `data.kind` of meerkat's reload-required refusal. */
export const MEMBER_RELOAD_REQUIRED_KIND = "mob_member_reload_required" as const;
/**
 * `meta/run_flow` refused its `roleTargets` (JSON-RPC invalid params,
 * `-32602`). The error data carries meerkat's typed refusal:
 * `{kind: FLOW_ROLE_TARGETS_REFUSED_KIND, flow_id, refusal: {kind, ...}}`.
 */
export const FLOW_ROLE_TARGETS_REFUSED_KIND = "flow_role_targets_refused" as const;
/**
 * `meta/run_flow` refused an `idempotency_key` already used with other
 * activation params or another binding (`-32602`). The error data is
 * `{kind: FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND, run_id, flow_id}`.
 */
export const FLOW_RUN_IDEMPOTENCY_CONFLICT_KIND = "flow_run_idempotency_conflict" as const;
/** The refusal kinds, a closed vocabulary: a new kind is a breaking change. */
export const FLOW_ROLE_TARGET_REFUSAL_KINDS: ReadonlySet<string> = new Set([
  "role_not_in_flow",
  "role_unbound",
  "quorum_exceeds_bound_set",
  "not_a_member",
  "role_mismatch",
  "not_active",
]);
/** WorkGraph service not configured on this runtime (memory-backend-unavailable pattern). */
export const WORKGRAPH_UNAVAILABLE_CODE = -32041 as const;
/**
 * A saved delivery scope no longer matches the identity's binding
 * (`meta/dispatch` with `expected_scope`): nothing was submitted and
 * nothing was retargeted.
 */
export const STALE_DELIVERY_SCOPE_CODE = -32007 as const;
/** WorkGraph CAS/revision conflict — refetch the item/binding's current revision and retry. */
export const WORKGRAPH_CONFLICT_CODE = -32042 as const;
/**
 * The identity's durable session cannot be resumed: Core reports it archived
 * and not revivable, or its durable authority contradictory. Final; nothing
 * was spawned. Core's mob machine and journal hold an identity's session and
 * generation, and the identity codes carry Core's typed answers.
 */
export const IDENTITY_UNRESUMABLE_CODE = -32051 as const;
/**
 * Core's member history, or the session resume, could not be read right now.
 * Retryable; nothing was spawned.
 */
export const IDENTITY_HISTORY_UNAVAILABLE_CODE = -32052 as const;
/**
 * The identity is managed by a host intent declaration in Core, so Meta's
 * lifecycle verbs refuse it.
 */
export const IDENTITY_HOST_ADOPTED_CODE = -32053 as const;
/**
 * A wake Core holds until an operator acts rides JSON-RPC's internal-error
 * code with structured data; the data `kind` names the hold.
 */
export const IDENTITY_WAKE_HELD_KINDS: ReadonlySet<string> = new Set([
  "mob_member_session_repair_required",
  "mob_member_session_resume_held",
  "identity_role_migration_refused",
]);

/**
 * Mob announcements (`meta/mob_announcements/*`) are not enabled on this
 * gateway. Every announcement refusal's data carries `kind`; an unknown kind
 * keeps its family's class.
 */
export const MOB_ANNOUNCEMENTS_UNAVAILABLE_CODE = -32044 as const;
/**
 * A conflicting or stale announcement request: other content under a source
 * key, a stale revision or cursor version, a reused operation id, a batch
 * that moved.
 */
export const ANNOUNCEMENT_CONFLICT_CODE = -32045 as const;
/** A requested start frontier lies before what a mob retains or beyond its head. */
export const ANNOUNCEMENT_FRONTIER_CODE = -32046 as const;
/** A typed mob announcement refusal; `kind` names it. */
export const ANNOUNCEMENT_REFUSED_CODE = -32047 as const;
/** The named subscription, delivery, source or mob does not exist here. */
export const ANNOUNCEMENT_NOT_FOUND_CODE = -32048 as const;

/**
 * Raised when the caller passes an `after_seq` past the current ledger
 * frontier. The server's `error.data` payload carries `after_cursor` and
 * `latest_cursor` — use the latter to rewind and resume.
 */
export class MobEventsStaleError extends RpcError {
  constructor(
    message: string,
    readonly afterCursor: number,
    readonly latestCursor: number,
    requestId: string,
    method: string,
    data?: unknown,
  ) {
    super(MOB_EVENTS_STALE_CURSOR_CODE, message, requestId, method, data);
    this.name = "MobEventsStaleError";
  }

  /**
   * Reify a generic {@link RpcError} with code `-32010` into the typed
   * form. Reads `after_cursor` / `latest_cursor` from the JSON-RPC
   * `error.data` payload; missing fields fall back to `0`.
   */
  static fromRpcError(err: RpcError): MobEventsStaleError {
    const payload =
      typeof err.data === "object" && err.data !== null
        ? (err.data as Record<string, unknown>)
        : {};
    const afterCursor = Number(payload.after_cursor ?? 0);
    const latestCursor = Number(payload.latest_cursor ?? 0);
    return new MobEventsStaleError(
      err.message,
      Number.isFinite(afterCursor) ? afterCursor : 0,
      Number.isFinite(latestCursor) ? latestCursor : 0,
      err.requestId,
      err.method,
      err.data,
    );
  }
}

// -- Capability errors ----------------------------------------------------

/** Raised when a requested capability is not available on the runtime. */
export class CapabilityUnavailableError extends RpcError {
  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(CAPABILITY_UNAVAILABLE_CODE, message, requestId, method, data);
    this.name = "CapabilityUnavailableError";
  }
}

/**
 * A send's `expectedSpecDigest` no longer names the identity's registered
 * row. The gateway checks it under the identity's lifecycle lock before
 * delivery, so nothing was submitted.
 */
export class RegisteredSpecChangedError extends RpcError {
  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(REGISTERED_SPEC_CHANGED_CODE, message, requestId, method, data);
    this.name = "RegisteredSpecChangedError";
  }
}

/**
 * Raised when a scoped dispatch's saved delivery scope no longer matches.
 * Nothing was submitted and the delivery was not retargeted to the
 * identity's current binding. `mismatch` names the atom that moved:
 * `runtime_id`, `generation`, `lease_fencing_token`, `member` or
 * `member_binding` (the member's session, incarnation or fence, or the
 * mob's incarnation). Recover an earlier attempt under the same scope with
 * `recoverDelivery`, never by redispatching to a fresh scope.
 */
export class StaleScopeError extends RpcError {
  readonly mismatch: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(STALE_DELIVERY_SCOPE_CODE, message, requestId, method, data);
    this.name = "StaleScopeError";
    const mismatch =
      data !== null && typeof data === "object"
        ? (data as Record<string, unknown>).mismatch
        : undefined;
    this.mismatch = typeof mismatch === "string" ? mismatch : null;
  }
}

export class MemoryBackendUnavailableError extends RpcError {
  constructor(
    message: string,
    requestId: string,
    method: string,
    data?: unknown,
  ) {
    super(MEMORY_BACKEND_UNAVAILABLE_CODE, message, requestId, method, data);
    this.name = "MemoryBackendUnavailableError";
  }
}

export class ConsoleTimelineReplayUnavailableError extends RpcError {
  constructor(
    message: string,
    requestId: string,
    method: string,
    data?: unknown,
  ) {
    super(CONSOLE_TIMELINE_REPLAY_UNAVAILABLE_CODE, message, requestId, method, data);
    this.name = "ConsoleTimelineReplayUnavailableError";
  }
}

/**
 * The gateway's storage cannot serve this call.
 *
 * At start the refusals are deliberate (storage-unification fail-closed
 * posture): file-name twins (e.g. `sessions.sqlite3` beside `sessions.db`)
 * the layout will not pick between, a session/runtime/blob/metadata/console
 * store that failed to open where older gateways silently fell back to
 * in-memory, or an uncreatable state root. The message names the
 * remediation: run the storage doctor (`meta/storage/doctor`) for twins, fix
 * the database file, or declare the ephemeral choice explicitly (e.g.
 * `runtimeStore.memory()`).
 *
 * After `storage-migrate-native`, the gateway's first boot carries out the
 * migration's first-boot manifest, and `kind` names its refusals:
 * - `"first_boot_manifest_unreadable"`: the manifest at `path` cannot be
 *   read, so no identity wakes;
 * - `"first_boot_row_refused"`: Core contradicts the manifest row for
 *   `identity` (`rowKind`, `manifestSession` and `coreSession` say how), so
 *   that identity's wakes and sends are refused;
 * - `"first_boot_row_pending"`: `operation` (`"reset"`, `"delete_identity"`
 *   or a delivery verb) waits until the row for `identity` is carried out.
 *   `waitingFor` says what it waits for, and `remedy` what to do:
 *   `"add_to_roster"` (no roster lists the identity) or `"wake_identity"`
 *   (the roster lists it and its row waits to be carried out). Retryable.
 *
 * `retryable` is `true` only when the gateway says so. The other fields are
 * `null` when the gateway did not send them; the full refusal stays
 * available as `data`.
 */
export class StorageResolutionError extends RpcError {
  readonly kind: string | null;
  readonly identity: string | null;
  readonly rowKind: string | null;
  readonly manifestSession: string | null;
  readonly coreSession: string | null;
  readonly operation: string | null;
  readonly path: string | null;
  readonly waitingFor: string | null;
  readonly remedy: string | null;
  readonly retryable: boolean;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(STORAGE_RESOLUTION_CODE, message, requestId, method, data);
    this.name = "StorageResolutionError";
    this.kind = dataString(data, "kind");
    this.identity = dataString(data, "identity");
    this.rowKind = dataString(data, "row_kind");
    this.manifestSession = dataString(data, "manifest_session");
    this.coreSession = dataString(data, "core_session");
    this.operation = dataString(data, "operation");
    this.path = dataString(data, "path");
    this.waitingFor = dataString(data, "waiting_for");
    this.remedy = dataString(data, "remedy");
    this.retryable =
      typeof data === "object" &&
      data !== null &&
      (data as Record<string, unknown>).retryable === true;
  }
}

/**
 * An ordinary request reached the gateway before `meta/init` settled. The
 * gateway serves ordinary requests only after init settles and refuses
 * earlier ones at once; startup provider callbacks must not issue ordinary
 * runtime RPCs on the runtime that is starting.
 */
export class InitInProgressError extends RpcError {
  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(INIT_IN_PROGRESS_CODE, message, requestId, method, data);
    this.name = "InitInProgressError";
  }
}

/**
 * The kinds of composition provenance refusal the gateway sends. A newer
 * gateway may send a kind not listed here, so `kind` is typed to admit any
 * string.
 */
export type CompositionProvenanceRefusalKind =
  | "divergent"
  | "candidate_divergent"
  | "created_by_rehearsal"
  | "missing"
  | "unreadable"
  | "malformed"
  | "unsupported_version"
  | "not_recorded"
  | "unproven_storage";

/**
 * Raised when the gateway refuses to start over the mob's composition
 * provenance.
 *
 * A persistent mob storage records the composition it was created for, and a
 * launch whose composition cannot be proven to match it is refused before the
 * mob actuates. The message names the remedy. `kind` names the refusal:
 *
 * - `"divergent"`: the supplied definition differs from the one recorded for
 *   the storage (revert the change, or move the stored definition with
 *   `MetaBuilder.declareSpecUpdate(expectedRevision)`);
 * - `"candidate_divergent"`: a candidate launch supplied a definition that
 *   differs from the stored one it would boot. Only a gateway launched with
 *   `runtime_options.mob_composition.authority = "candidate"` raises it,
 *   which this builder never sends; the gateway option
 *   `runtime_options.mob_composition.candidate_definition = "stored"`
 *   acknowledges the stored definition;
 * - `"created_by_rehearsal"`: the storage was created by a launch that did
 *   not speak for the durable composition;
 * - `"unreadable"`, `"malformed"`, `"unsupported_version"` or
 *   `"not_recorded"`: the provenance record beside the storage is unusable,
 *   or could not be written;
 * - `"missing"`: no provenance record beside non-empty storage. The gateway
 *   does not raise it today (an authoritative launch adopts and records the
 *   definition instead); library callers can see it;
 * - `"unproven_storage"`: non-empty storage with nothing declared about it.
 *
 * `kind` is `null` when the gateway sent no refusal data. `fields` lists the
 * diverged definition fields as dotted paths (`profiles.lead.tools.deny`);
 * it is empty unless the kind is `divergent` or `candidate_divergent`. The
 * full refusal stays available as `data`.
 */
export class CompositionProvenanceError extends RpcError {
  readonly kind: CompositionProvenanceRefusalKind | (string & {}) | null;
  readonly fields: readonly string[];

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(COMPOSITION_PROVENANCE_CODE, message, requestId, method, data);
    this.name = "CompositionProvenanceError";
    const refusal =
      typeof data === "object" && data !== null && !Array.isArray(data)
        ? (data as Record<string, unknown>)
        : {};
    this.kind = typeof refusal.kind === "string" ? refusal.kind : null;
    this.fields = Array.isArray(refusal.fields)
      ? refusal.fields.filter((field): field is string => typeof field === "string")
      : [];
  }
}

/**
 * Raised when a member needs a runtime reload before it accepts work.
 *
 * `meta/force_cancel_member` answers meerkat's typed refusal (code `-32029`,
 * data `kind` `"mob_member_reload_required"`) for a member whose runtime
 * registration cannot serve work. Both must match; any other `-32029` answer
 * stays a plain `RpcError`. `memberId` and
 * `reason` come from the refusal's data, and `requiredAction` names meerkat's
 * repair, `"reload_member_registration"`. Each is `null` when the gateway sent
 * no such field; the full refusal stays available as `data`.
 */
export class MemberReloadRequiredError extends RpcError {
  readonly memberId: string | null;
  readonly reason: string | null;
  readonly requiredAction: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(MEMBER_RELOAD_REQUIRED_CODE, message, requestId, method, data);
    this.name = "MemberReloadRequiredError";
    const refusal =
      typeof data === "object" && data !== null && !Array.isArray(data)
        ? (data as Record<string, unknown>)
        : {};
    this.memberId = typeof refusal.member_id === "string" ? refusal.member_id : null;
    this.reason = typeof refusal.reason === "string" ? refusal.reason : null;
    this.requiredAction =
      typeof refusal.required_action === "string" ? refusal.required_action : null;
  }
}

/**
 * `meta/run_flow` refused its `roleTargets`; no run started.
 *
 * The gateway answers invalid params (`-32602`) with data `kind`
 * `"flow_role_targets_refused"`; both must match, and any other `-32602`
 * answer stays a plain {@link RpcError}. `flowId` names the flow, and
 * `refusal` is meerkat's typed refusal: its `kind` is one of
 * {@link FLOW_ROLE_TARGET_REFUSAL_KINDS} (`role_not_in_flow`, `role_unbound`,
 * `quorum_exceeds_bound_set`, `not_a_member`, `role_mismatch`, `not_active`)
 * beside the role, member and other fields that kind names. `refusalKind` is
 * that `kind`, `null` when the gateway sent none.
 */
export class FlowRoleTargetsRefusedError extends RpcError {
  readonly flowId: string | null;
  readonly refusal: Readonly<Record<string, unknown>>;
  readonly refusalKind: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(-32602, message, requestId, method, data);
    this.name = "FlowRoleTargetsRefusedError";
    const payload =
      typeof data === "object" && data !== null && !Array.isArray(data)
        ? (data as Record<string, unknown>)
        : {};
    this.flowId = typeof payload.flow_id === "string" ? payload.flow_id : null;
    const refusal = payload.refusal;
    this.refusal =
      typeof refusal === "object" && refusal !== null && !Array.isArray(refusal)
        ? { ...(refusal as Record<string, unknown>) }
        : {};
    this.refusalKind =
      typeof this.refusal.kind === "string" ? this.refusal.kind : null;
  }
}

/**
 * `meta/run_flow` refused an `idempotency_key` that already admitted a run
 * with other activation params or another binding; the first run stands and
 * no second run was admitted.
 *
 * The gateway answers invalid params (`-32602`) with data `kind`
 * `"flow_run_idempotency_conflict"`; both must match, and any other `-32602`
 * answer stays a plain {@link RpcError}. `runId` names the run the key
 * admitted and `flowId` its flow, each `null` when the gateway sent none.
 */
export class FlowRunIdempotencyConflictError extends RpcError {
  readonly runId: string | null;
  readonly flowId: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(-32602, message, requestId, method, data);
    this.name = "FlowRunIdempotencyConflictError";
    const payload =
      typeof data === "object" && data !== null && !Array.isArray(data)
        ? (data as Record<string, unknown>)
        : {};
    this.runId = typeof payload.run_id === "string" ? payload.run_id : null;
    this.flowId = typeof payload.flow_id === "string" ? payload.flow_id : null;
  }
}

/**
 * `meta/run_flow` started a run without echoing the `roleTargets` it was
 * sent.
 *
 * A gateway that does not echo the binding may have run the flow role-wide,
 * so the SDK asked it to cancel the run before throwing. `runId` names the
 * run, `requested` is the binding sent, `echoed` the response's
 * `role_targets` (`undefined` when absent), and `cancelError` the error the
 * cancel threw, `null` when it was accepted (when it is set, the run may
 * still be running).
 */
export class FlowRoleTargetsNotAppliedError extends MetaError {
  constructor(
    message: string,
    readonly runId: string,
    readonly requested: Readonly<Record<string, readonly string[]>>,
    readonly echoed: unknown,
    readonly cancelError: unknown = null,
  ) {
    super(message);
    this.name = "FlowRoleTargetsNotAppliedError";
  }
}

/** Raised when a `meta/workgraph/*` call is made but no WorkGraph service is configured. */
export class WorkGraphUnavailableError extends RpcError {
  constructor(
    message: string,
    requestId: string,
    method: string,
    data?: unknown,
  ) {
    super(WORKGRAPH_UNAVAILABLE_CODE, message, requestId, method, data);
    this.name = "WorkGraphUnavailableError";
  }
}

/**
 * Raised on a WorkGraph CAS/revision conflict (the caller's `expected_revision`
 * is stale). `data.detail` carries the upstream message. Refetch the item or
 * attention binding's current revision and retry.
 */
export class WorkGraphConflictError extends RpcError {
  constructor(
    message: string,
    requestId: string,
    method: string,
    data?: unknown,
  ) {
    super(WORKGRAPH_CONFLICT_CODE, message, requestId, method, data);
    this.name = "WorkGraphConflictError";
  }
}

function dataString(data: unknown, key: string): string | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const value = (data as Record<string, unknown>)[key];
  return typeof value === "string" ? value : null;
}

/**
 * The identity's durable session cannot be resumed. The refusal is final.
 *
 * Core still holds the identity's history, but cannot resume the session it
 * names (`reason` is `"archived_not_revivable"` or
 * `"contradictory_durable_authority"`; a newer reason is kept as sent).
 * Nothing was spawned, and a retry ends the same way. The remedy (`remedy`,
 * `"delete_identity"`) starts the identity on a fresh session. `identity`,
 * `sessionId`, `reason` and `remedy` are `null` when the gateway sent no
 * refusal data; the full refusal stays available as `data`.
 */
export class IdentityUnresumableError extends RpcError {
  readonly retryable = false;
  readonly identity: string | null;
  readonly sessionId: string | null;
  readonly reason:
    | "archived_not_revivable"
    | "contradictory_durable_authority"
    | (string & {})
    | null;
  readonly remedy: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(IDENTITY_UNRESUMABLE_CODE, message, requestId, method, data);
    this.name = "IdentityUnresumableError";
    this.identity = dataString(data, "identity");
    this.sessionId = dataString(data, "session_id");
    this.reason = dataString(data, "reason");
    this.remedy = dataString(data, "remedy");
  }
}

/**
 * Core's member history or the session resume could not be read right now.
 *
 * Retryable: nothing was spawned and the identity's history is untouched, so
 * the same call can be retried. `kind` is `"identity_history_unavailable"`
 * when Core's history read failed, `"identity_wake_retryable"` when the
 * wake failed transiently (resuming `sessionId`, or starting fresh when
 * `sessionId` is `null`), or `"identity_session_absent"` when a delivery
 * reached a member whose session `sessionId` Core holds no durable copy of:
 * nothing was delivered, and the next delivery starts the identity's fresh
 * session. A failed read of the whole roster's history names no `identity`.
 * `kind`, `identity` and `sessionId` are `null` when the gateway did not send
 * them.
 */
export class IdentityHistoryUnavailableError extends RpcError {
  readonly retryable = true;
  readonly kind: string | null;
  readonly identity: string | null;
  readonly sessionId: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(IDENTITY_HISTORY_UNAVAILABLE_CODE, message, requestId, method, data);
    this.name = "IdentityHistoryUnavailableError";
    this.kind = dataString(data, "kind");
    this.identity = dataString(data, "identity");
    this.sessionId = dataString(data, "session_id");
  }
}

/**
 * The identity is managed by a host intent declaration in Core.
 *
 * Core holds intent for the identity (the host adopted it), so Meta's
 * lifecycle verbs refuse to act on it; the host's reconciler owns it.
 * `identity` is `null` when the gateway did not send it.
 */
export class IdentityHostAdoptedError extends RpcError {
  readonly retryable = false;
  readonly identity: string | null;

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(IDENTITY_HOST_ADOPTED_CODE, message, requestId, method, data);
    this.name = "IdentityHostAdoptedError";
    this.identity = dataString(data, "identity");
  }
}

/**
 * The identity's wake is held until an operator acts.
 *
 * Core kept the identity's session and its content but will not resume it on
 * its own, and a retry ends the same way. The identity reads `dormant` and
 * its status carries the refusal. `kind` names the hold:
 *
 * - `"mob_member_session_repair_required"`: the session's committed document
 *   fails Core's audited-endpoint check; `operatorPath` lists the repair, then
 *   `meta/reload_member`;
 * - `"mob_member_session_resume_held"`: Core holds the session's resume;
 *   `durableResumeHold` or `resumeUnavailableReason` names why;
 * - `"identity_role_migration_refused"`: the roster changed the identity's
 *   profile without a declared role migration.
 *
 * `kind`, `identity`, `sessionId`, `durableResumeHold` and
 * `resumeUnavailableReason` are `null` when the gateway did not send them; the
 * full refusal stays available as `data`.
 */
export class IdentityWakeHeldError extends RpcError {
  readonly retryable = false;
  readonly kind: string | null;
  readonly identity: string | null;
  readonly sessionId: string | null;
  readonly durableResumeHold: string | null;
  readonly resumeUnavailableReason: string | null;
  readonly operatorPath: readonly string[];

  constructor(
    message: string,
    requestId = "",
    method = "",
    data?: unknown,
  ) {
    super(-32603, message, requestId, method, data);
    this.name = "IdentityWakeHeldError";
    this.kind = dataString(data, "kind");
    this.identity = dataString(data, "identity");
    this.sessionId = dataString(data, "session_id");
    this.durableResumeHold = dataString(data, "durable_resume_hold");
    this.resumeUnavailableReason = dataString(data, "resume_unavailable_reason");
    const operatorPath =
      typeof data === "object" && data !== null && !Array.isArray(data)
        ? (data as Record<string, unknown>).operator_path
        : undefined;
    this.operatorPath = Array.isArray(operatorPath)
      ? operatorPath.filter((step): step is string => typeof step === "string")
      : [];
  }
}

// -- Contract errors ------------------------------------------------------

/** Raised when the SDK and runtime contract versions are incompatible. */
function dataInteger(data: unknown, key: string): number | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const value = (data as Record<string, unknown>)[key];
  return typeof value === "number" && Number.isInteger(value) ? value : null;
}

function dataRecord(data: unknown, key: string): Record<string, unknown> | null {
  if (typeof data !== "object" || data === null || Array.isArray(data)) return null;
  const value = (data as Record<string, unknown>)[key];
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

/** Mob announcements are not enabled on this gateway (`-32044`). */
export class MobAnnouncementsUnavailableError extends RpcError {
  /** The refusal's `data.kind`, or `null` when the gateway sent none. */
  readonly kind: string | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(MOB_ANNOUNCEMENTS_UNAVAILABLE_CODE, message, requestId, method, data);
    this.name = "MobAnnouncementsUnavailableError";
    this.kind = dataString(data, "kind");
  }
}

/**
 * A conflicting or stale announcement request (`-32045`). `kind` is
 * `"announcement_conflict"` (other content under the source key),
 * `"stale_revision"`, `"operation_reused"`, `"batch_moved"`,
 * `"source_version_conflict"` or newer. A source's stale revision and version
 * conflict are the subclasses, which carry the source's current position;
 * read it again and retry from there.
 */
export class AnnouncementConflictError extends RpcError {
  /** The refusal's `data.kind`, or `null` when the gateway sent none. */
  readonly kind: string | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(ANNOUNCEMENT_CONFLICT_CODE, message, requestId, method, data);
    this.name = "AnnouncementConflictError";
    this.kind = dataString(data, "kind");
  }
}

/**
 * A source change or collector step named a revision that is no longer
 * current. Carries the source's current revision, cursor version, generation
 * and cursor (as sent).
 */
export class SourceStaleRevisionError extends AnnouncementConflictError {
  readonly currentRevision: number | null;
  readonly currentVersion: number | null;
  readonly generation: number | null;
  readonly cursor: Record<string, unknown> | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(message, requestId, method, data);
    this.name = "SourceStaleRevisionError";
    this.currentRevision = dataInteger(data, "current_revision");
    this.currentVersion = dataInteger(data, "current_version");
    this.generation = dataInteger(data, "generation");
    this.cursor = dataRecord(data, "cursor");
  }
}

/**
 * A collector step named a cursor version that is no longer current: another
 * step committed first. Carries the source's current version, cursor (as
 * sent), state and revision.
 */
export class SourceVersionConflictError extends AnnouncementConflictError {
  readonly currentVersion: number | null;
  readonly cursor: Record<string, unknown> | null;
  readonly state: string | null;
  readonly revision: number | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(message, requestId, method, data);
    this.name = "SourceVersionConflictError";
    this.currentVersion = dataInteger(data, "current_version");
    this.cursor = dataRecord(data, "cursor");
    this.state = dataString(data, "state");
    this.revision = dataInteger(data, "revision");
  }
}

/**
 * A start frontier outside a mob's announcements (`-32046`): `kind` is
 * `"gap"` (before what the mob retains) or `"frontier_beyond_head"`.
 */
export class AnnouncementFrontierError extends RpcError {
  /** The refusal's `data.kind`, or `null` when the gateway sent none. */
  readonly kind: string | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(ANNOUNCEMENT_FRONTIER_CODE, message, requestId, method, data);
    this.name = "AnnouncementFrontierError";
    this.kind = dataString(data, "kind");
  }
}

/** A typed mob announcement refusal (`-32047`); `kind` names it. */
export class AnnouncementRefusedError extends RpcError {
  /** The refusal's `data.kind`, or `null` when the gateway sent none. */
  readonly kind: string | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(ANNOUNCEMENT_REFUSED_CODE, message, requestId, method, data);
    this.name = "AnnouncementRefusedError";
    this.kind = dataString(data, "kind");
  }
}

/** The named subscription, delivery, source or mob does not exist here (`-32048`). */
export class AnnouncementNotFoundError extends RpcError {
  /** The refusal's `data.kind`, or `null` when the gateway sent none. */
  readonly kind: string | null;

  constructor(message: string, requestId = "", method = "", data?: unknown) {
    super(ANNOUNCEMENT_NOT_FOUND_CODE, message, requestId, method, data);
    this.name = "AnnouncementNotFoundError";
    this.kind = dataString(data, "kind");
  }
}

/**
 * The typed mob announcement error for a JSON-RPC error, or `null` when the
 * code is not one of the family's (`-32044` to `-32048`).
 */
export function parseAnnouncementRpcError(
  code: number,
  message: string,
  requestId = "",
  method = "",
  data?: unknown,
): RpcError | null {
  const kind = dataString(data, "kind");
  switch (code) {
    case MOB_ANNOUNCEMENTS_UNAVAILABLE_CODE:
      return new MobAnnouncementsUnavailableError(message, requestId, method, data);
    case ANNOUNCEMENT_CONFLICT_CODE:
      if (kind === "source_version_conflict") {
        return new SourceVersionConflictError(message, requestId, method, data);
      }
      // A source's stale revision carries its cursor position; a
      // subscription's carries only the current revision.
      if (kind === "stale_revision" && dataInteger(data, "current_version") !== null) {
        return new SourceStaleRevisionError(message, requestId, method, data);
      }
      return new AnnouncementConflictError(message, requestId, method, data);
    case ANNOUNCEMENT_FRONTIER_CODE:
      return new AnnouncementFrontierError(message, requestId, method, data);
    case ANNOUNCEMENT_REFUSED_CODE:
      return new AnnouncementRefusedError(message, requestId, method, data);
    case ANNOUNCEMENT_NOT_FOUND_CODE:
      return new AnnouncementNotFoundError(message, requestId, method, data);
    default:
      return null;
  }
}

export class ContractMismatchError extends MetaError {
  constructor(message: string) {
    super(message);
    this.name = "ContractMismatchError";
  }
}

// -- Connection errors ----------------------------------------------------

/** Raised when an operation requires a connected runtime but none is available. */
export class NotConnectedError extends MetaError {
  constructor(message: string) {
    super(message);
    this.name = "NotConnectedError";
  }
}

// -- Ticketed turns -------------------------------------------------------

/**
 * The turn a ticket names ran and failed (`meta/turn_result` state
 * `failed`). `reason` carries the runtime's typed detail.
 */
export class TurnFailedError extends MetaError {
  readonly identity: string;
  readonly ticket: string;
  readonly reason: string;
  /** The send/dispatch result when an `*AndWait` call threw this, else `null`. */
  admission: unknown = null;

  constructor(identity: string, ticket: string, reason: string) {
    super(`turn ${ticket} of identity ${identity} failed: ${reason}`);
    this.name = "TurnFailedError";
    this.identity = identity;
    this.ticket = ticket;
    this.reason = reason;
  }
}

/**
 * The gateway knows no turn with this ticket for this identity: never
 * admitted there, another identity's, aged out, or the gateway restarted
 * since. Never guessed in either direction.
 */
/**
 * A server-side wait ended, typed, before what it waited for happened.
 * `outcome` is the gateway's token: `run_failed` (a run failed or was
 * cancelled during the wait), `broken`, `retiring` (retiring, retired or
 * being deleted), `identity_gone` or `shutting_down`. Tolerate future values.
 */
export class WaitEndedError extends MetaError {
  readonly identity: string;
  readonly outcome: string;
  /** The send/dispatch result when an `*AndWait` call threw this, else `null`. */
  admission: unknown = null;
  /**
   * The ticket of the tracked turn the `*AndWait` call was waiting on, else
   * `null` (an untracked wait, or not thrown by an `*AndWait` call).
   */
  ticket: string | null = null;

  constructor(identity: string, outcome: string, detail = "") {
    const message = `wait on identity ${identity} ended: ${outcome}`;
    super(detail ? `${message} (${detail})` : message);
    this.name = "WaitEndedError";
    this.identity = identity;
    this.outcome = outcome;
  }
}

export class TurnUnknownError extends MetaError {
  readonly identity: string;
  readonly ticket: string;
  /**
   * The send/dispatch result when an `*AndWait` call threw this after its
   * delivery was admitted, else `null`: do not redispatch the delivery
   * because its ticket is no longer known.
   */
  admission: unknown = null;

  constructor(identity: string, ticket: string) {
    super(
      `no turn ${ticket} is known for identity ${identity} (never admitted ` +
        `there, aged out, or the gateway restarted)`,
    );
    this.name = "TurnUnknownError";
    this.identity = identity;
    this.ticket = ticket;
  }
}

/**
 * A `trackTurn` delivery was NOT delivered at all (`turnUnavailable.code ===
 * "not_delivered"`: the gateway has no session bridge, or the identity has no
 * bound runtime). Nothing ran and there is no turn to wait for, so retrying
 * is safe, unlike a failed or unknown turn.
 */
export class TurnNotDeliveredError extends MetaError {
  readonly identity: string;
  readonly operation: string;
  readonly code: string;
  readonly reason: string;
  /** The send/dispatch result that reported it, when an `*AndWait` call threw this. */
  admission: unknown = null;

  constructor(identity: string, operation: string, code: string, reason: string) {
    super(
      `${operation} for identity ${identity} was not delivered ` +
        `(${code}: ${reason}); there is no turn to wait for`,
    );
    this.name = "TurnNotDeliveredError";
    this.identity = identity;
    this.operation = operation;
    this.code = code;
    this.reason = reason;
  }
}

/**
 * An `*AndWait` call made with `requireAttribution: true` could not track its
 * delivery's own turn, so it refused to wait identity-wide.
 *
 * The delivery was ADMITTED: the work was handed to the identity and will
 * run, or already ran. Keep `admission` (the full send/dispatch result) and
 * do not redispatch it. `code` and `reason` carry the typed `turnUnavailable`
 * reason (`autonomous_host`, `runtime_refused`, ...), or are `null` when the
 * gateway predates turn tickets. Without `requireAttribution` the call waits
 * identity-wide instead and resolves a non-attributed result.
 */
export class TurnTrackingUnavailableError extends MetaError {
  readonly identity: string;
  readonly operation: string;
  readonly admission: unknown;
  readonly code: string | null;
  readonly reason: string | null;

  constructor(
    identity: string,
    operation: string,
    admission: unknown,
    code: string | null,
    reason: string | null,
  ) {
    const detail = code !== null ? `${code}: ${reason}` : "no turn ticket returned";
    super(
      `${operation} for identity ${identity} was admitted but its turn ` +
        `cannot be tracked (${detail}); the admission result is retained, ` +
        `do not redispatch`,
    );
    this.name = "TurnTrackingUnavailableError";
    this.identity = identity;
    this.operation = operation;
    this.admission = admission;
    this.code = code;
    this.reason = reason;
  }
}

/**
 * Observing an ADMITTED delivery's turn failed for a transport or RPC reason
 * (the gateway did not answer the wait), and retrying that exact observation
 * ran out of the caller's deadline.
 *
 * The turn may still run, or may already have completed. `admission` is the
 * full send/dispatch result and `ticket` names the turn (`null` only on the
 * identity-wide wait). Read the turn later with `waitForTurn(ticket)` or
 * `turnResult(ticket)`; never redispatch the business work because of this
 * error. `cause` is the last observation failure and `attempts` counts
 * observation tries.
 */
export class PostAdmissionObservationError extends MetaError {
  readonly identity: string;
  readonly operation: string;
  readonly admission: unknown;
  readonly ticket: string | null;
  readonly attempts: number;

  constructor(
    identity: string,
    operation: string,
    admission: unknown,
    ticket: string | null,
    attempts: number,
    cause: unknown,
  ) {
    const turn = ticket !== null ? `turn ${ticket}` : "its turn";
    super(
      `${operation} for identity ${identity} was admitted, but observing ` +
        `${turn} failed after ${attempts} attempt(s); the admission result is ` +
        `retained, do not redispatch`,
      { cause },
    );
    this.name = "PostAdmissionObservationError";
    this.identity = identity;
    this.operation = operation;
    this.admission = admission;
    this.ticket = ticket;
    this.attempts = attempts;
  }
}

/**
 * A ticketed turn did not settle by the caller's deadline. From an
 * `*AndWait` call it carries `admission` (the send/dispatch result) so the
 * turn can be read later instead of redispatched.
 */
export class TurnWaitTimeoutError extends MetaError {
  readonly identity: string;
  readonly ticket: string;
  readonly timeoutMs: number;
  /** The send/dispatch result when an `*AndWait` call threw this, else `null`. */
  admission: unknown = null;

  constructor(identity: string, ticket: string, timeoutMs: number) {
    super(
      `turn ${ticket} of identity ${identity} did not complete within ` +
        `${timeoutMs}ms`,
    );
    this.name = "TurnWaitTimeoutError";
    this.identity = identity;
    this.ticket = ticket;
    this.timeoutMs = timeoutMs;
  }
}

// -- Cross-module identity helpers ---------------------------------------

/**
 * Structural test for an `RpcError`. Pre-fix every site used
 * `err instanceof RpcError`, but JS class identity is module-scoped:
 * dual CJS+ESM packaging, vitest module isolation, and hoisted-vs-
 * nested workspace deps can produce two `RpcError` constructors that
 * fail `instanceof` for each other's instances. The structural check
 * survives those splits.
 */
export function isRpcError(err: unknown): err is RpcError {
  if (err instanceof RpcError) {
    return true;
  }
  if (err === null || typeof err !== "object") {
    return false;
  }
  const candidate = err as { name?: unknown; code?: unknown };
  return (
    candidate.name === "RpcError" ||
    candidate.name === "MobEventsStaleError" ||
    candidate.name === "CapabilityUnavailableError" ||
    candidate.name === "MemoryBackendUnavailableError" ||
    candidate.name === "ConsoleTimelineReplayUnavailableError" ||
    candidate.name === "StorageResolutionError" ||
    candidate.name === "CompositionProvenanceError" ||
    candidate.name === "WorkGraphUnavailableError" ||
    candidate.name === "WorkGraphConflictError"
  ) && typeof candidate.code === "number";
}

/** Structural test for a `MobEventsStaleError`. See `isRpcError`. */
export function isMobEventsStaleError(err: unknown): err is MobEventsStaleError {
  if (err instanceof MobEventsStaleError) {
    return true;
  }
  return (
    isRpcError(err) &&
    err.code === MOB_EVENTS_STALE_CURSOR_CODE &&
    (err as { name: string }).name === "MobEventsStaleError"
  );
}

// -- Backward compatibility -----------------------------------------------

/** @deprecated Use {@link RpcError} instead. */
export const MetaRpcError = RpcError;
/** @deprecated Use {@link RpcError} instead. */
export type MetaRpcError = RpcError;
