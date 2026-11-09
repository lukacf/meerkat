/** Explicit trusted-host protocol7 client. Native facts are historical observations,
 * never reusable credential coverage, a Build permit or a permission grant. */
import {
  ServiceTransport, ServiceSetupError, ServiceSetupEvent, EVENTS, COMPLETIONS,
  bindingWire, serviceEnvironment, object, keys, text, path, utf8, jsonSafe, copy,
  type ObjectValue, type ServiceProtocol, type PreparedServiceLaunch,
  type ServiceSetupBinding,
} from "./service-transport.js";
export { ServiceSetupError, ServiceSetupTimeout, ServiceSetupEnded, ServiceSetupLaunchError,
  ServiceSetupEvent } from "./service-transport.js";
export type { ServiceSetupBinding, ServiceSetupAnswer, ServiceSetupClose } from "./service-transport.js";

/** Selected by application host code. This never constructs native authority.
 * The embedding must bind real logout to ServiceProbeControl::retire. No flags,
 * credentials, sources or fallback executable are inferred by this client. */
export interface ServiceCollectionLauncher {
  readonly protocol_version: 7;
  readonly executable: string;
  readonly args: readonly string[];
  readonly credential_scope: ServiceSetupBinding["credential_scope"];
  readonly environment?: Readonly<Record<string, string>>;
}
import type { ServiceProbeReport, ServiceProbeResource, ServiceProbeStatus } from "./service-probe.js";
export type ServiceCollectionResource = ServiceProbeResource;
export type ServiceCollectionStatus = "historical_checks_completed" | "not_verified" | "not_observed" | "native_stopped"
  | "governed_prerequisite_unavailable" | "native_refused" | "outcome_unknown" | "tool_failed" | "cleanup_unconfirmed"
  | "unsupported" | "selection_changed" | "cancelled" | "program_refused" | "program_panicked"
  | "attempt_already_observed" | "replay_refused" | "budget_exceeded";
export interface CollectionLimits {
  readonly calls: number; readonly argument_bytes: number; readonly result_bytes: number;
}
export interface CollectionCallObservation {
  readonly source_id: string; readonly required_operation_id: string;
  readonly resource: ServiceCollectionResource; readonly ordinal: number; readonly raw_operation: string;
  readonly arguments_sha256: string; readonly native: ServiceProbeReport;
}
export interface RequiredOperationObservation {
  readonly required_operation_id: string; readonly resource: ServiceCollectionResource;
  readonly raw_operation_ceiling: readonly string[];
  readonly assessment: "verified" | "not_verified" | "unknown" | null;
  readonly calls: readonly CollectionCallObservation[];
}
export interface ServiceCollectionReport {
  readonly source_id: string; readonly attempt_id: string;
  readonly application_id: string; readonly instance_id: string; readonly login_scope_id: string;
  readonly viewer_id: string; readonly origin: string; readonly realm: string;
  readonly session_id: string; readonly job_id: string; readonly family: string;
  readonly profile: string; readonly agent: string; readonly status: ServiceCollectionStatus;
  readonly requirements: readonly RequiredOperationObservation[];
  readonly observed_selection: "not_observed" | "rechecked_after_last_close" | "changed_or_unconfirmed";
  readonly native_own_config_generation: number | null;
  readonly reusable_source_coverage: "unqualified_credential_continuity";
  readonly limits: CollectionLimits;
  readonly total_calls: number; readonly total_argument_bytes: number; readonly total_result_bytes: number;
}
export interface ServiceCommissioning {
  readonly realm: string; readonly session_id: string; readonly job_id: string; readonly agent: string;
  readonly state: "completed" | "incomplete";
  readonly governance: "not_entered" | "entered" | "committed" | "outcome_unknown" | "already_governed";
  readonly grants: readonly {
    readonly state: "committed" | "refused" | "issued_not_selected" | "issuance_unknown" | "selection_unknown" | "outcome_unknown";
    readonly issued: string | null; readonly candidate_grant_id: string | null; readonly superseded: string | null;
    readonly cleanup: readonly ("revoked" | "controller_in_use" | "deferred" | "references_unknown" | "custody_unavailable" | "refused" | "uncertain")[];
  }[];
  readonly controller_disposal: "not_prepared" | "drained" | "taken" | "already_retired" | null;
  readonly issuance: "not_entered" | "committed" | "uncertain" | null;
  readonly custody_released: boolean;
  readonly capability_verified: false;
}
export interface ServiceCollectionState {
  readonly available: boolean; readonly can_send: boolean; readonly can_stop: boolean;
  readonly outcome_uncertain: boolean; readonly private_notice_pending: boolean;
  readonly active_send: { readonly send_request_id: string; readonly cancellation_requested: boolean } | null;
  readonly last_completion: { readonly send_request_id: string; readonly completion: string } | null;
  readonly commissioning: boolean; readonly can_collect: boolean;
  readonly active_collection: string | null; readonly collection_sources: readonly string[] | null;
  readonly reusable_source_coverage: "unqualified_credential_continuity";
  readonly service_ids?: readonly string[];
  readonly service_id?: string;
  readonly can_open_next_service?: boolean;
}

const STATUSES = new Set<ServiceCollectionStatus>(["historical_checks_completed", "not_verified", "not_observed", "native_stopped",
  "governed_prerequisite_unavailable", "native_refused", "outcome_unknown", "tool_failed", "cleanup_unconfirmed",
  "unsupported", "selection_changed", "cancelled", "program_refused", "program_panicked", "attempt_already_observed",
  "replay_refused", "budget_exceeded"]);
const COVERAGE = "unqualified_credential_continuity";
const COLLECTION_CAPABILITIES = Object.freeze({ selection: "trusted_process_sources",
  requires: "successful_finalization_and_governed_reopen", result: "historical_required_operation_observations",
  reusable_source_coverage: COVERAGE, slack_inbound: "unsupported" });
const REPORT_STRINGS = ["source_id", "attempt_id", "application_id", "instance_id", "login_scope_id", "viewer_id", "origin",
  "realm", "session_id", "job_id", "family", "profile", "agent"];
const NATIVE_STRINGS = ["profile_id", "application_id", "instance_id", "login_scope_id", "viewer_id", "origin",
  "realm", "session_id", "job_id", "server", "agent", "operation"];
const NATIVE_NULLABLE = ["input_id", "run_id", "tool_call_id", "result_sha256", "installed_connection_sha256", "dispatch_outcome"];
const NATIVE_STATUSES: readonly ServiceProbeStatus[] = ["governed_prerequisite_unavailable", "selection_changed", "not_observed",
  "native_refused", "outcome_unknown", "tool_failed", "tool_completed", "capability_not_verified", "capability_verified",
  "cancelled", "cleanup_unconfirmed", "unsupported"];
const STATE_BOOLEANS = ["available", "can_send", "can_stop", "outcome_uncertain", "private_notice_pending", "commissioning", "can_collect"];
function require(value: unknown): asserts value { if (!value) throw new ServiceSetupError("invalid native service collection observation"); }
function nullableText(value: unknown): boolean { return value === null || text(value, 4096); }
function oneOf(value: unknown, choices: readonly (string | null)[]): boolean {
  return (typeof value === "string" || value === null) && choices.includes(value);
}
function hash(value: unknown): boolean { return value === null || typeof value === "string" && /^[0-9a-f]{64}$/u.test(value); }

function integer(value: unknown, maximum = Number.MAX_SAFE_INTEGER): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 && value <= maximum;
}
function canonical(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (object(value)) return `{${Object.keys(value).sort().map(k => `${JSON.stringify(k)}:${canonical(value[k])}`).join(",")}}`;
  const encoded = JSON.stringify(value); require(typeof encoded === "string"); return encoded;
}
function resource(value: unknown): boolean { return value !== null && Buffer.byteLength(JSON.stringify(value)) <= 4096; }
function nativeReport(value: unknown, expected: ObjectValue): Readonly<ServiceProbeReport> {
  require(object(value) && keys(value, [...NATIVE_STRINGS, ...NATIVE_NULLABLE, "resource", "status", "native_terminal_committed", "native_close_joined"]));
  require(NATIVE_STRINGS.every(k => text(value[k], 4096))
    && Object.entries(expected).every(([k, v]) => canonical(value[k]) === canonical(v))
    && typeof value.status === "string" && NATIVE_STATUSES.includes(value.status as ServiceProbeStatus)
    && [value.input_id, value.run_id, value.tool_call_id].every(nullableText)
    && hash(value.result_sha256) && hash(value.installed_connection_sha256)
    && oneOf(value.dispatch_outcome, [null, "committed", "failed", "unknown"])
    && typeof value.native_terminal_committed === "boolean" && typeof value.native_close_joined === "boolean");
  if (["tool_completed", "capability_verified", "capability_not_verified"].includes(value.status)) {
    require(value.native_terminal_committed && value.native_close_joined && value.dispatch_outcome === "committed"
      && [value.input_id, value.run_id, value.tool_call_id, value.result_sha256, value.installed_connection_sha256].every(v => v !== null));
  }
  if (value.dispatch_outcome === "unknown") require(oneOf(value.status, ["outcome_unknown", "cancelled", "cleanup_unconfirmed"]));
  return value as unknown as Readonly<ServiceProbeReport>;
}

function launch(launcher: ServiceCollectionLauncher, binding: ServiceSetupBinding): PreparedServiceLaunch {
  try {
    const initial = bindingWire(binding);
    jsonSafe(launcher);
    require(object(launcher) && keys(launcher, ["protocol_version", "executable", "args", "credential_scope"], ["environment"]));
    require(launcher.protocol_version === 7 && path(launcher.executable));
    require(Array.isArray(launcher.args) && launcher.args.length <= 128
      && launcher.args.every(a => (a === "" || utf8(a, 16384)) && !a.includes("\0"))
      && Buffer.byteLength(JSON.stringify(launcher.args)) <= 65536);
    const selected = launcher.credential_scope, expected = binding.credential_scope;
    require(object(selected) && (expected.kind === "file"
      ? keys(selected, ["kind", "root"]) && selected.kind === "file" && selected.root === expected.root
      : keys(selected, ["kind"]) && selected.kind === "native_default"));
    const env = serviceEnvironment(binding, launcher.environment);
    initial.protocol_version = 7;
    return { executable: launcher.executable, args: [...launcher.args], env, initial,
      deadline: binding.expires_at_ms, version: 7 };
  } catch { throw new ServiceSetupError("explicit trusted protocol7 launcher and matching credential policy required"); }
}

/** Only the transport invokes this observer. Its bookkeeping correlates native
 * observations; it neither grants access nor substitutes resource validation. */
class CollectionProtocol implements ServiceProtocol {
  readonly errorCodes = new Set(["collection_unavailable", "collection_already_attempted"]);
  readonly events = new Set([...EVENTS, "service_collection_state", "service_commissioning"]);
  readonly #identity: ObjectValue;
  #session: string | undefined;
  #scope: string | undefined;
  #serviceIds: readonly string[] | undefined;
  #serviceId: string | undefined;
  #nextService: { id: string; service: string } | undefined;
  #sessions = new Set<string>();
  #jobs = new Set<string>();
  #pendingRequests = new Set<string>();
  #firstFence: ObjectValue | undefined;
  #sources: readonly string[] | undefined;
  #job: string | undefined;
  #commissionJob: string | undefined;
  #commissionAgent: string | undefined;
  #setupClosed = false;
  #unavailable = false;
  #active: { id: string; job: string; source: string; observed: boolean } | undefined;
  #lastCompleted: string | undefined;
  #attempted = new Set<string>();
  #refusedSources = new Map<string, Set<string>>();
  #attemptIds = new Set<string>();
  #state: Readonly<ServiceCollectionState> | undefined;
  readonly #reports = new WeakMap<ServiceSetupEvent, Readonly<ServiceCollectionReport>>();
  readonly #commissions = new WeakMap<ServiceSetupEvent, Readonly<ServiceCommissioning>>();
  readonly #current = new WeakMap<ServiceSetupEvent, boolean>();
  constructor(initial: ObjectValue) {
    const app = initial.application as ObjectValue, viewer = initial.viewer as ObjectValue, realm = initial.realm as ObjectValue;
    this.#identity = copy({ application_id: app.application_id, instance_id: app.instance_id, origin: app.origin,
      viewer_id: viewer.principal_id, login_scope_id: viewer.login_scope_id, realm: realm.realm_id });
  }
  get state(): Readonly<ServiceCollectionState> | undefined { return this.#state; }
  get unavailable(): boolean { return this.#unavailable; }
  currentAtObservation(event: ServiceSetupEvent): boolean { return this.#current.get(event) ?? false; }
  report(event: ServiceSetupEvent): Readonly<ServiceCollectionReport> | undefined { return this.#reports.get(event); }
  commissioning(event: ServiceSetupEvent): Readonly<ServiceCommissioning> | undefined { return this.#commissions.get(event); }
  beforeRequest(method: string, params: ObjectValue): void {
    if (method === "connect/send" || method.startsWith("connect/connector/")) {
      if (this.#setupClosed || this.#unavailable || (method === "connect/send" ? !this.#state?.can_send : !this.#state?.available)) {
        throw new ServiceSetupError("native setup input owner closed or unavailable");
      }
    }
    if (method === "connect/collect") {
      if (!this.#state?.can_collect || this.#unavailable || this.#active || this.#nextService || this.#job === undefined || this.#job !== params.job_id
          || !text(params.source_id) || !this.#sources?.includes(params.source_id)
          || this.#attempted.has(params.source_id)
          || this.#refusedSources.get(this.#job)?.has(params.source_id)) throw new ServiceSetupError("exact native collection unavailable or already attempted");
    }
    if (method === "connect/collection/open_next") {
      const index = this.#serviceIds?.indexOf(this.#serviceId ?? "") ?? -1;
      if (!this.#state?.can_open_next_service || this.#unavailable || this.#nextService
          || this.#pendingRequests.size || this.#active || index < 0 || !text(params.service_id)
          || this.#serviceIds?.[index + 1] !== params.service_id) {
        throw new ServiceSetupError("next preselected native service unavailable");
      }
    }
  }
  queued(id: string, method: string, params: ObjectValue): void {
    this.#pendingRequests.add(id);
    if (method === "connect/collection/open_next") this.#nextService = { id, service: params.service_id as string };
    if (method === "connect/collect") {
      this.#active = { id, job: params.job_id as string, source: params.source_id as string, observed: false };
      this.#attempted.add(params.source_id as string);
    }
  }
  cancellable(id: string): boolean {
    // Queuing the next setup ends the preceding collection's cancel window,
    // even while its final state is still buffered on the original pipe.
    return !this.#unavailable && !this.#nextService && (this.#active?.id === id || !this.#active && this.#lastCompleted === id);
  }
  retires(params: ObjectValue): boolean {
    require((keys(params, ["kind", "target", "reason"]) || keys(params, ["kind", "target", "reason", "first_fence"])) && text(params.reason));
    if (Object.hasOwn(params, "first_fence")) {
      const fence = params.first_fence;
      // Native diagnostic labels can grow independently of this SDK. Preserve
      // bounded text without interpreting it as authority or cleanup evidence.
      require(object(fence) && keys(fence, ["origin", "scope", "send_request_id", "cancellation_requested_before"])
        && text(fence.origin)
        && fence.scope === (this.#scope ?? null) && (fence.send_request_id === null || text(fence.send_request_id))
        && (fence.cancellation_requested_before === null || typeof fence.cancellation_requested_before === "boolean"));
      if (this.#firstFence) require(Object.keys(fence).every(key => fence[key] === this.#firstFence?.[key]));
      else this.#firstFence = copy(fence);
    }
    // Native setup shutdown announces finalizing before opening the distinct
    // commissioning presenter. This exception never reverses an earlier fence.
    if (params.reason === "finalizing" && this.#session !== undefined) {
      this.#setupClosed = true;
      return false;
    }
    this.#unavailable = true;
    return true;
  }
  #acceptState(value: unknown, opened = false): void {
    const fields = [...STATE_BOOLEANS, "active_send", "last_completion", "active_collection", "collection_sources", "reusable_source_coverage"];
    require(object(value) && (keys(value, fields) || keys(value, [...fields, "service_ids", "service_id", "can_open_next_service"])));
    if (Object.hasOwn(value, "service_ids")) {
      require(Array.isArray(value.service_ids) && value.service_ids.length >= 2 && value.service_ids.length <= 16
        && value.service_ids.every(id => text(id)) && new Set(value.service_ids).size === value.service_ids.length
        && text(value.service_id) && value.service_ids.includes(value.service_id) && typeof value.can_open_next_service === "boolean");
      const ids = value.service_ids as string[], current = value.service_id, pending = this.#nextService?.service;
      if (this.#serviceIds) require(ids.length === this.#serviceIds.length && ids.every((id, i) => id === this.#serviceIds?.[i]));
      require(current === (this.#serviceId ?? ids[0]) || current === pending);
      require(!value.can_open_next_service || current !== ids.at(-1) && value.can_collect === true && current !== pending);
      if (current === pending && current !== this.#serviceId) {
        // Observe the actual phase change before replacing phase-local facts.
        // Buffered state from the preceding phase is legal until this point.
        this.#session = this.#job = this.#commissionJob = this.#commissionAgent = undefined;
        this.#setupClosed = false;
        this.#lastCompleted = undefined;
      }
      this.#serviceIds = ids; this.#serviceId = current;
    } else require(this.#serviceIds === undefined);
    require(STATE_BOOLEANS.every(k => typeof value[k] === "boolean") && value.reusable_source_coverage === COVERAGE);
    require(value.active_send === null || object(value.active_send) && keys(value.active_send, ["send_request_id", "cancellation_requested"])
      && text(value.active_send.send_request_id) && typeof value.active_send.cancellation_requested === "boolean");
    require(value.last_completion === null || object(value.last_completion) && keys(value.last_completion, ["send_request_id", "completion"])
      && text(value.last_completion.send_request_id) && typeof value.last_completion.completion === "string" && COMPLETIONS.has(value.last_completion.completion));
    require(value.active_collection === null || value.active_collection === this.#active?.id);
    if (this.#session === undefined && !opened) require(value.available === false && value.can_send === false
      && value.can_stop === false && value.commissioning === false && value.can_collect === false
      && value.active_collection === null && value.active_send === null && value.last_completion === null);
    if (value.collection_sources === null) require(this.#sources === undefined && value.can_collect === false && value.active_collection === null);
    else {
      require(Array.isArray(value.collection_sources) && value.collection_sources.length >= 1 && value.collection_sources.length <= 16
        && value.collection_sources.every(p => text(p)) && new Set(value.collection_sources).size === value.collection_sources.length);
      const sources = value.collection_sources as string[];
      if (this.#sources) require(this.#sources.length === sources.length && this.#sources.every((p, i) => p === sources[i]));
      else this.#sources = sources;
    }
    require(!this.#setupClosed || value.available === false && value.can_send === false);
    require(!value.commissioning || this.#setupClosed && value.can_send === false && value.can_collect === false);
    require(!value.can_collect || this.#job !== undefined && value.active_collection === null && value.commissioning === false);
    this.#state = value as unknown as Readonly<ServiceCollectionState>;
    if (value.active_collection !== null && this.#active) this.#active.observed = true;
  }
  #acceptCommission(value: unknown): Readonly<ServiceCommissioning> {
    require(object(value) && keys(value, ["realm", "session_id", "job_id", "agent", "state", "governance", "grants",
      "controller_disposal", "issuance", "custody_released", "capability_verified"]));
    require(this.#setupClosed && value.realm === this.#identity.realm && value.session_id === this.#session
      && text(value.job_id) && text(value.agent) && value.capability_verified === false && typeof value.custody_released === "boolean");
    require(oneOf(value.state, ["completed", "incomplete"])
      && oneOf(value.governance, ["not_entered", "entered", "committed", "outcome_unknown", "already_governed"])
      && oneOf(value.controller_disposal, [null, "not_prepared", "drained", "taken", "already_retired"])
      && oneOf(value.issuance, [null, "not_entered", "committed", "uncertain"]));
    require(Array.isArray(value.grants) && value.grants.length <= 2);
    for (const grant of value.grants) {
      require(object(grant) && keys(grant, ["state", "issued", "candidate_grant_id", "superseded", "cleanup"])
        && oneOf(grant.state, ["committed", "refused", "issued_not_selected", "issuance_unknown", "selection_unknown", "outcome_unknown"])
        && [grant.issued, grant.candidate_grant_id, grant.superseded].every(nullableText)
        && Array.isArray(grant.cleanup) && grant.cleanup.length <= 256
        && grant.cleanup.every(v => oneOf(v, ["revoked", "controller_in_use", "deferred", "references_unknown", "custody_unavailable", "refused", "uncertain"])));
    }
    require(this.#commissionJob === undefined);
    this.#commissionJob = value.job_id;
    this.#commissionAgent = value.agent;
    return value as unknown as Readonly<ServiceCommissioning>;
  }
  #acceptReport(value: unknown, requestId: string | null): Readonly<ServiceCollectionReport> {
    require(this.#active && this.#active.id === requestId);
    const active = this.#active;
    require(object(value) && keys(value, [...REPORT_STRINGS, "status", "requirements", "observed_selection", "native_own_config_generation",
      "reusable_source_coverage", "limits", "total_calls", "total_argument_bytes", "total_result_bytes"]));
    require(REPORT_STRINGS.every(k => text(value[k], 4096))
      && Object.entries(this.#identity).every(([k, v]) => value[k] === v)
      && value.source_id === active.source && value.job_id === active.job && value.session_id === this.#session
      && (this.#commissionAgent === undefined || value.agent === this.#commissionAgent)
      && typeof value.attempt_id === "string" && /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/u.test(value.attempt_id)
      && !this.#attemptIds.has(value.attempt_id)
      && typeof value.status === "string" && STATUSES.has(value.status as ServiceCollectionStatus)
      && value.reusable_source_coverage === COVERAGE
      && oneOf(value.observed_selection, ["not_observed", "rechecked_after_last_close", "changed_or_unconfirmed"])
      && (value.native_own_config_generation === null || integer(value.native_own_config_generation)));
    const limits = value.limits;
    require(object(limits) && keys(limits, ["calls", "argument_bytes", "result_bytes"])
      && integer(limits.calls, 256) && limits.calls >= 1
      && integer(limits.argument_bytes, 8 * 1024 * 1024) && limits.argument_bytes >= 1
      && integer(limits.result_bytes, 32 * 1024 * 1024) && limits.result_bytes >= 1
      && integer(value.total_calls, limits.calls) && integer(value.total_argument_bytes, limits.argument_bytes)
      && integer(value.total_result_bytes) && Array.isArray(value.requirements)
      && value.requirements.length >= 1 && value.requirements.length <= 64);
    const ids = new Set<string>(), callKeys = new Set<string>(), nativeCalls = new Set<string>(); let count = 0;
    for (const row of value.requirements) {
      require(object(row) && keys(row, ["required_operation_id", "resource", "raw_operation_ceiling", "assessment", "calls"])
        && text(row.required_operation_id) && !ids.has(row.required_operation_id) && resource(row.resource)
        && oneOf(row.assessment, [null, "verified", "not_verified", "unknown"])
        && Array.isArray(row.raw_operation_ceiling) && row.raw_operation_ceiling.length >= 1 && row.raw_operation_ceiling.length <= 32
        && row.raw_operation_ceiling.every(v => text(v)) && new Set(row.raw_operation_ceiling).size === row.raw_operation_ceiling.length
        && Array.isArray(row.calls) && row.calls.length <= limits.calls);
      ids.add(row.required_operation_id);
      for (const call of row.calls) {
        count++;
        require(object(call) && keys(call, ["source_id", "required_operation_id", "resource", "ordinal", "raw_operation", "arguments_sha256", "native"])
          && call.source_id === value.source_id && call.required_operation_id === row.required_operation_id
          && canonical(call.resource) === canonical(row.resource) && integer(call.ordinal) && call.ordinal === count
          && typeof call.raw_operation === "string" && row.raw_operation_ceiling.includes(call.raw_operation)
          && typeof call.arguments_sha256 === "string" && hash(call.arguments_sha256));
        const native = nativeReport(call.native, { ...this.#identity, session_id: this.#session, job_id: active.job,
          agent: value.agent, profile_id: row.required_operation_id, operation: call.raw_operation, resource: row.resource });
        const tuple = [native.input_id, native.run_id, native.tool_call_id], correlation = canonical(tuple);
        // Native ReobserveRead may repeat the arguments with a new observation,
        // including partial admission on failure. Dedup observations here;
        // only the native owner can admit a read.
        const key = canonical([row.required_operation_id, call.raw_operation, call.arguments_sha256, tuple]);
        require(!callKeys.has(key) && (tuple.includes(null) || !nativeCalls.has(correlation)));
        callKeys.add(key); if (!tuple.includes(null)) nativeCalls.add(correlation);
        if (row.assessment === "verified") require(native.status === "tool_completed");
      }
      if (row.assessment === "verified") require(row.calls.length > 0);
    }
    // Reserved calls may lack a produced row. An observed result may exceed the
    // budget; preserve that count and the native refusal, never replay it.
    require(count <= value.total_calls);
    if (value.status === "historical_checks_completed") require(count === value.total_calls
      && value.requirements.every(r => object(r) && r.assessment === "verified")
      && value.observed_selection === "rechecked_after_last_close" && value.native_own_config_generation !== null
      && value.total_result_bytes <= limits.result_bytes);
    this.#attemptIds.add(value.attempt_id);
    return value as unknown as Readonly<ServiceCollectionReport>;
  }
  observe(event: ServiceSetupEvent, viewerCurrent: boolean): void {
    const p = event.payload;
    if (event.kind === "result") {
      const result = p.result;
      require(object(result));
      if (p.method === "connect/initialize") {
        require(this.#session === undefined && object(result.capabilities));
        const caps = result.capabilities.service_collections;
        require(object(caps) && keys(caps, Object.keys(COLLECTION_CAPABILITIES))
          && Object.entries(COLLECTION_CAPABILITIES).every(([k, v]) => caps[k] === v));
        require(text(result.session_id));
        this.#session = result.session_id;
        require(text(result.scope)); this.#scope = result.scope;
        this.#sessions.add(this.#session);
        this.#acceptState(result.state);
      } else if (p.method === "connect/collection/open_next") {
        require(this.#nextService?.id === event.request_id
          && keys(result, ["scope", "service_id", "session_id", "state"])
          && result.scope === this.#scope && result.service_id === this.#nextService?.service
          && text(result.session_id) && !this.#sessions.has(result.session_id)
          && object(result.state) && result.state.service_id === result.service_id);
        this.#acceptState(result.state, true);
        this.#session = result.session_id; this.#sessions.add(this.#session);
        this.#nextService = undefined;
      } else if (p.method === "connect/collect") {
        require(keys(result, ["scope", "collection_request_id", "report", "state"]) && result.collection_request_id === event.request_id);
        const report = this.#acceptReport(result.report, event.request_id);
        this.#acceptState(result.state);
        require(this.#state?.active_collection === null);
        this.#reports.set(event, report);
        this.#lastCompleted = event.request_id as string;
        this.#active = undefined;
      } else if (p.method === "connect/send") this.#acceptState(result.state);
    } else if (event.kind === "refused") {
      if (p.method === "connect/collection/open_next") {
        require(this.#nextService?.id === event.request_id);
        this.#nextService = undefined; this.#unavailable = true;
      }
      if (p.method === "connect/collect") {
        require(this.#active?.id === event.request_id);
        const active = this.#active;
        if (p.code === "invalid_binding" && !active.observed) {
          // begin_collection refuses a foreign service source before admission.
          // Block the same job/source, not a later native installation's choice.
          const refused = this.#refusedSources.get(active.job) ?? new Set<string>();
          refused.add(active.source); this.#refusedSources.set(active.job, refused);
          this.#attempted.delete(active.source);
        }
        this.#active = undefined; // Other refusals retain the burned source.
      }
    } else if (event.kind === "state") {
      // The native initializer publishes state before Work::Opened returns a
      // session. The shared transport already checked its exact initial target.
      require(keys(p, ["kind", "target", "state"]));
      this.#acceptState(p.state);
    } else if (event.kind === "service_collection_state") {
      require(this.#session !== undefined && this.#setupClosed && p.capability_verified === false);
      if (p.state === "available" || p.state === "retired") {
        require(keys(p, ["kind", "target", "state", "job_id", "session_id", "capability_verified"])
          && text(p.job_id) && p.session_id === this.#session
          && (this.#job === undefined || this.#job === p.job_id)
          && (this.#commissionJob === undefined || this.#commissionJob === p.job_id));
        require(this.#job !== undefined || !this.#jobs.has(p.job_id));
        this.#jobs.add(p.job_id); this.#job = p.job_id;
        if (p.state === "retired") this.#unavailable = true;
      } else {
        require(this.#job === undefined && keys(p, ["kind", "target", "state", "capability_verified"])
          && oneOf(p.state, ["unsupported_source", "finalization_not_selected"]));
        this.#unavailable = true;
      }
    } else if (event.kind === "service_commissioning") {
      require(keys(p, ["kind", "target", "state", "observation", "capability_verified"])
        && this.#session !== undefined && p.state === "observed" && p.capability_verified === false);
      this.#commissions.set(event, this.#acceptCommission(p.observation));
    } else if (event.kind === "prompt" && this.#setupClosed) {
      require(this.#state?.commissioning === true && p.kind === "approval");
    }
    if ((event.kind === "result" || event.kind === "refused") && event.request_id) this.#pendingRequests.delete(event.request_id);
    this.#current.set(event, viewerCurrent && !this.#unavailable);
  }
}

/** One process for setup, commissioning, finalization and exact native collections.
 * Timeout means observe this same owner again. No implicit retry or downgrade. */
export class ServiceCollectionClient extends ServiceTransport {
  readonly #protocol: CollectionProtocol;
  constructor(launcher: ServiceCollectionLauncher, binding: ServiceSetupBinding) {
    const selected = launch(launcher, binding), protocol = new CollectionProtocol(selected.initial);
    super(selected, protocol);
    this.#protocol = protocol;
  }
  /** Last immutable native projection, not current resource authorization. */
  get state(): Readonly<ServiceCollectionState> | undefined { return this.#protocol.state; }
  /** Local currentness now, never an authorization or a resource capability. */
  get viewerCurrent(): boolean { return this.transportCurrent() && !this.#protocol.unavailable; }
  /** Historical local currentness when this exact owned event was read. Later
   * logout, cancellation, expiry or close does not rewrite historical facts. */
  viewerCurrentAtObservation(event: ServiceSetupEvent): boolean { return this.#protocol.currentAtObservation(event); }
  collect(selection: { readonly job_id: string; readonly source_id: string }): string {
    try { jsonSafe(selection); require(object(selection) && keys(selection, ["job_id", "source_id"])
      && text(selection.job_id) && text(selection.source_id)); }
    catch { throw new ServiceSetupError("closed opaque native collection selection required"); }
    return this.scopedRequest("connect/collect", { job_id: selection.job_id, source_id: selection.source_id });
  }
  /** Open the next preselected setup on this same process and scope. This
   * sends no caller, grant, receipt or program and never retries a transition. */
  openNextService(selection: { readonly service_id: string }): string {
    try { jsonSafe(selection); require(object(selection) && keys(selection, ["service_id"]) && text(selection.service_id)); }
    catch { throw new ServiceSetupError("exact observed next service required"); }
    return this.scopedRequest("connect/collection/open_next", { service_id: selection.service_id });
  }
  /** Typed historical facts only from this client's validated event stream.
   * Another client or a constructed event never supplies a valid observation. */
  collectionReport(event: ServiceSetupEvent): Readonly<ServiceCollectionReport> | undefined { return this.#protocol.report(event); }
  commissioning(event: ServiceSetupEvent): Readonly<ServiceCommissioning> | undefined { return this.#protocol.commissioning(event); }
}
