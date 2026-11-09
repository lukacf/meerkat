/** Explicit trusted-host protocol6 client. Native facts are private observations,
 * not a Build permit, permission grant, or proof of another resource. */
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
 * credentials, profiles or fallback executable are inferred by this client. */
export interface ServiceProbeLauncher {
  readonly protocol_version: 6;
  readonly executable: string;
  readonly args: readonly string[];
  readonly credential_scope: ServiceSetupBinding["credential_scope"];
  readonly environment?: Readonly<Record<string, string>>;
}
export type ServiceProbeResource = null | boolean | number | string
  | readonly ServiceProbeResource[] | { readonly [key: string]: ServiceProbeResource };
export type ServiceProbeStatus = "governed_prerequisite_unavailable" | "selection_changed" | "not_observed"
  | "native_refused" | "outcome_unknown" | "tool_failed" | "tool_completed" | "capability_not_verified"
  | "capability_verified" | "cancelled" | "cleanup_unconfirmed" | "unsupported";
export interface ServiceProbeReport {
  readonly profile_id: string; readonly application_id: string; readonly instance_id: string;
  readonly login_scope_id: string; readonly viewer_id: string; readonly origin: string;
  readonly realm: string; readonly session_id: string; readonly job_id: string;
  readonly server: string; readonly agent: string; readonly operation: string;
  readonly resource: ServiceProbeResource;
  readonly status: ServiceProbeStatus;
  readonly input_id: string | null; readonly run_id: string | null; readonly tool_call_id: string | null;
  readonly result_sha256: string | null; readonly installed_connection_sha256: string | null;
  readonly dispatch_outcome: "committed" | "failed" | "unknown" | null;
  readonly native_terminal_committed: boolean; readonly native_close_joined: boolean;
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
export interface ServiceProbeState {
  readonly available: boolean; readonly can_send: boolean; readonly can_stop: boolean;
  readonly outcome_uncertain: boolean; readonly private_notice_pending: boolean;
  readonly active_send: { readonly send_request_id: string; readonly cancellation_requested: boolean } | null;
  readonly last_completion: { readonly send_request_id: string; readonly completion: string } | null;
  readonly commissioning: boolean; readonly can_probe: boolean;
  readonly active_probe: string | null; readonly probe_profiles: readonly string[];
}

const STATUSES = new Set<ServiceProbeStatus>(["governed_prerequisite_unavailable", "selection_changed", "not_observed",
  "native_refused", "outcome_unknown", "tool_failed", "tool_completed", "capability_not_verified",
  "capability_verified", "cancelled", "cleanup_unconfirmed", "unsupported"]);
const PROBE_CAPABILITIES = Object.freeze({ selection: "trusted_process_profiles",
  requires: "successful_finalization_and_governed_reopen", result: "native_tool_completion_or_profile_verified_resource",
  slack_inbound: "unsupported" });
const REPORT_STRINGS = ["profile_id", "application_id", "instance_id", "login_scope_id", "viewer_id", "origin",
  "realm", "session_id", "job_id", "server", "agent", "operation"];
const REPORT_NULLABLE = ["input_id", "run_id", "tool_call_id", "result_sha256", "installed_connection_sha256", "dispatch_outcome"];
const STATE_BOOLEANS = ["available", "can_send", "can_stop", "outcome_uncertain", "private_notice_pending", "commissioning", "can_probe"];
function require(value: unknown): asserts value { if (!value) throw new ServiceSetupError("invalid native service probe observation"); }
function nullableText(value: unknown): boolean { return value === null || text(value, 4096); }
function oneOf(value: unknown, choices: readonly (string | null)[]): boolean {
  return (typeof value === "string" || value === null) && choices.includes(value);
}
function hash(value: unknown): boolean { return value === null || typeof value === "string" && /^[0-9a-f]{64}$/u.test(value); }

function launch(launcher: ServiceProbeLauncher, binding: ServiceSetupBinding): PreparedServiceLaunch {
  try {
    const initial = bindingWire(binding);
    jsonSafe(launcher);
    require(object(launcher) && keys(launcher, ["protocol_version", "executable", "args", "credential_scope"], ["environment"]));
    require(launcher.protocol_version === 6 && path(launcher.executable));
    require(Array.isArray(launcher.args) && launcher.args.length <= 128
      && launcher.args.every(a => (a === "" || utf8(a, 16384)) && !a.includes("\0"))
      && Buffer.byteLength(JSON.stringify(launcher.args)) <= 65536);
    const selected = launcher.credential_scope, expected = binding.credential_scope;
    require(object(selected) && (expected.kind === "file"
      ? keys(selected, ["kind", "root"]) && selected.kind === "file" && selected.root === expected.root
      : keys(selected, ["kind"]) && selected.kind === "native_default"));
    const env = serviceEnvironment(binding, launcher.environment);
    initial.protocol_version = 6;
    return { executable: launcher.executable, args: [...launcher.args], env, initial,
      deadline: binding.expires_at_ms, version: 6 };
  } catch { throw new ServiceSetupError("explicit trusted protocol6 launcher and matching credential policy required"); }
}

/** Only the transport invokes this observer. Its bookkeeping correlates native
 * observations; it neither grants access nor substitutes resource validation. */
class ProbeProtocol implements ServiceProtocol {
  readonly events = new Set([...EVENTS, "service_probe_state", "service_commissioning"]);
  readonly #identity: ObjectValue;
  #session: string | undefined;
  #profiles: readonly string[] | undefined;
  #job: string | undefined;
  #commissionJob: string | undefined;
  #commissionAgent: string | undefined;
  #setupClosed = false;
  #unavailable = false;
  #active: { id: string; job: string; profile: string } | undefined;
  #lastCompleted: string | undefined;
  #attempted = new Set<string>();
  #state: Readonly<ServiceProbeState> | undefined;
  readonly #reports = new WeakMap<ServiceSetupEvent, Readonly<ServiceProbeReport>>();
  readonly #commissions = new WeakMap<ServiceSetupEvent, Readonly<ServiceCommissioning>>();
  readonly #current = new WeakMap<ServiceSetupEvent, boolean>();
  constructor(initial: ObjectValue) {
    const app = initial.application as ObjectValue, viewer = initial.viewer as ObjectValue, realm = initial.realm as ObjectValue;
    this.#identity = copy({ application_id: app.application_id, instance_id: app.instance_id, origin: app.origin,
      viewer_id: viewer.principal_id, login_scope_id: viewer.login_scope_id, realm: realm.realm_id });
  }
  get state(): Readonly<ServiceProbeState> | undefined { return this.#state; }
  get unavailable(): boolean { return this.#unavailable; }
  currentAtObservation(event: ServiceSetupEvent): boolean { return this.#current.get(event) ?? false; }
  report(event: ServiceSetupEvent): Readonly<ServiceProbeReport> | undefined { return this.#reports.get(event); }
  commissioning(event: ServiceSetupEvent): Readonly<ServiceCommissioning> | undefined { return this.#commissions.get(event); }
  beforeRequest(method: string, params: ObjectValue): void {
    if (method === "connect/send" || method.startsWith("connect/connector/")) {
      if (this.#setupClosed || this.#unavailable || (method === "connect/send" ? !this.#state?.can_send : !this.#state?.available)) {
        throw new ServiceSetupError("native setup input owner closed or unavailable");
      }
    }
    if (method === "connect/probe") {
      if (!this.#state?.can_probe || this.#unavailable || this.#active || this.#job !== params.job_id
          || !text(params.profile_id) || !this.#profiles?.includes(params.profile_id)
          || this.#attempted.has(params.profile_id)) throw new ServiceSetupError("exact native probe unavailable or already attempted");
    }
  }
  queued(id: string, method: string, params: ObjectValue): void {
    if (method === "connect/probe") {
      this.#active = { id, job: params.job_id as string, profile: params.profile_id as string };
      this.#attempted.add(params.profile_id as string);
    }
  }
  cancellable(id: string): boolean { return this.#active?.id === id || !this.#active && this.#lastCompleted === id; }
  retires(params: ObjectValue): boolean {
    require(keys(params, ["kind", "target", "reason"]) && text(params.reason));
    // Native setup shutdown announces finalizing before opening the distinct
    // commissioning presenter. This exception never reverses an earlier fence.
    if (params.reason === "finalizing" && this.#session !== undefined) {
      this.#setupClosed = true;
      return false;
    }
    this.#unavailable = true;
    return true;
  }
  #acceptState(value: unknown): void {
    require(object(value) && keys(value, [...STATE_BOOLEANS, "active_send", "last_completion", "active_probe", "probe_profiles"]));
    require(STATE_BOOLEANS.every(k => typeof value[k] === "boolean"));
    require(value.active_send === null || object(value.active_send) && keys(value.active_send, ["send_request_id", "cancellation_requested"])
      && text(value.active_send.send_request_id) && typeof value.active_send.cancellation_requested === "boolean");
    require(value.last_completion === null || object(value.last_completion) && keys(value.last_completion, ["send_request_id", "completion"])
      && text(value.last_completion.send_request_id) && typeof value.last_completion.completion === "string" && COMPLETIONS.has(value.last_completion.completion));
    require(value.active_probe === null || value.active_probe === this.#active?.id);
    if (this.#session === undefined) require(value.available === false && value.can_send === false
      && value.can_stop === false && value.commissioning === false && value.can_probe === false
      && value.active_probe === null && value.active_send === null && value.last_completion === null);
    require(Array.isArray(value.probe_profiles) && value.probe_profiles.length >= 1 && value.probe_profiles.length <= 16
      && value.probe_profiles.every(p => text(p)) && new Set(value.probe_profiles).size === value.probe_profiles.length);
    const profiles = value.probe_profiles as string[];
    if (this.#profiles) require(this.#profiles.length === profiles.length && this.#profiles.every((p, i) => p === profiles[i]));
    else this.#profiles = profiles;
    require(!this.#setupClosed || value.can_send === false);
    require(!value.commissioning || this.#setupClosed && value.can_send === false && value.can_probe === false);
    require(!value.can_probe || this.#job !== undefined && value.active_probe === null && value.commissioning === false);
    this.#state = value as unknown as Readonly<ServiceProbeState>;
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
  #acceptReport(value: unknown, requestId: string | null): Readonly<ServiceProbeReport> {
    require(this.#active && this.#active.id === requestId);
    const active = this.#active;
    require(object(value) && keys(value, [...REPORT_STRINGS, ...REPORT_NULLABLE, "resource", "status", "native_terminal_committed", "native_close_joined"]));
    require(REPORT_STRINGS.every(k => text(value[k], k === "origin" ? 4096 : 256))
      && Object.entries(this.#identity).every(([k, expected]) => value[k] === expected)
      && value.profile_id === active.profile && value.job_id === active.job && value.session_id === this.#session);
    require(this.#commissionAgent === undefined || value.agent === this.#commissionAgent);
    require(typeof value.status === "string" && STATUSES.has(value.status as ServiceProbeStatus));
    require([value.input_id, value.run_id, value.tool_call_id].every(nullableText)
      && hash(value.result_sha256) && hash(value.installed_connection_sha256)
      && oneOf(value.dispatch_outcome, [null, "committed", "failed", "unknown"])
      && typeof value.native_terminal_committed === "boolean" && typeof value.native_close_joined === "boolean"
      && value.resource !== null && Buffer.byteLength(JSON.stringify(value.resource)) <= 4096);
    if (["tool_failed", "tool_completed", "capability_verified", "capability_not_verified"].includes(value.status)) {
      require(value.native_terminal_committed && value.native_close_joined
        && [value.input_id, value.run_id, value.tool_call_id, value.result_sha256, value.installed_connection_sha256].every(v => v !== null)
        && oneOf(value.dispatch_outcome, ["committed", "failed"]));
    }
    if (["tool_completed", "capability_verified", "capability_not_verified"].includes(value.status)) require(value.dispatch_outcome === "committed");
    if (value.dispatch_outcome === "unknown") require(oneOf(value.status, ["outcome_unknown", "cancelled", "cleanup_unconfirmed"]));
    // Retirement can change a status to cancelled while preserving actual facts.
    // Neither a fact nor its serialization revives current authorization.
    return value as unknown as Readonly<ServiceProbeReport>;
  }
  observe(event: ServiceSetupEvent, viewerCurrent: boolean): void {
    const p = event.payload;
    if (event.kind === "result") {
      const result = p.result;
      require(object(result));
      if (p.method === "connect/initialize") {
        require(this.#session === undefined && object(result.capabilities));
        const caps = result.capabilities.service_probes;
        require(object(caps) && keys(caps, Object.keys(PROBE_CAPABILITIES))
          && Object.entries(PROBE_CAPABILITIES).every(([k, v]) => caps[k] === v));
        require(text(result.session_id));
        this.#session = result.session_id;
        this.#acceptState(result.state);
      } else if (p.method === "connect/probe") {
        require(keys(result, ["scope", "probe_request_id", "report", "state"]) && result.probe_request_id === event.request_id);
        const report = this.#acceptReport(result.report, event.request_id);
        this.#acceptState(result.state);
        require(this.#state?.active_probe === null);
        this.#reports.set(event, report);
        this.#lastCompleted = event.request_id as string;
        this.#active = undefined;
      } else if (p.method === "connect/send") this.#acceptState(result.state);
    } else if (event.kind === "refused") {
      if (p.method === "connect/probe") {
        require(this.#active?.id === event.request_id);
        this.#active = undefined; // Attempt remains burned even on refusal.
      }
    } else if (event.kind === "state") {
      // The native initializer publishes state before Work::Opened returns a
      // session. The shared transport already checked its exact initial target.
      require(keys(p, ["kind", "target", "state"]));
      this.#acceptState(p.state);
    } else if (event.kind === "service_probe_state") {
      require(this.#session !== undefined && this.#setupClosed && p.capability_verified === false);
      if (p.state === "available" || p.state === "retired") {
        require(keys(p, ["kind", "target", "state", "job_id", "session_id", "capability_verified"])
          && text(p.job_id) && p.session_id === this.#session
          && (this.#job === undefined || this.#job === p.job_id)
          && (this.#commissionJob === undefined || this.#commissionJob === p.job_id));
        this.#job = p.job_id;
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
    this.#current.set(event, viewerCurrent && !this.#unavailable);
  }
}

/** One process for setup, commissioning, finalization and exact native probes.
 * Timeout means observe this same owner again. No implicit retry or downgrade. */
export class ServiceProbeClient extends ServiceTransport {
  readonly #protocol: ProbeProtocol;
  constructor(launcher: ServiceProbeLauncher, binding: ServiceSetupBinding) {
    const selected = launch(launcher, binding), protocol = new ProbeProtocol(selected.initial);
    super(selected, protocol);
    this.#protocol = protocol;
  }
  /** Last immutable native projection, not current resource authorization. */
  get state(): Readonly<ServiceProbeState> | undefined { return this.#protocol.state; }
  /** Local currentness now, never an authorization or a resource capability. */
  get viewerCurrent(): boolean { return this.transportCurrent() && !this.#protocol.unavailable; }
  /** Historical local currentness when this exact owned event was read. Later
   * logout, cancellation, expiry or close does not rewrite historical facts. */
  viewerCurrentAtObservation(event: ServiceSetupEvent): boolean { return this.#protocol.currentAtObservation(event); }
  probe(selection: { readonly job_id: string; readonly profile_id: string }): string {
    try { jsonSafe(selection); require(object(selection) && keys(selection, ["job_id", "profile_id"])
      && text(selection.job_id) && text(selection.profile_id)); }
    catch { throw new ServiceSetupError("closed opaque native probe selection required"); }
    return this.scopedRequest("connect/probe", { job_id: selection.job_id, profile_id: selection.profile_id });
  }
  /** Typed historical facts only from this client's validated event stream.
   * Another client or a constructed event never supplies a valid observation. */
  probeReport(event: ServiceSetupEvent): Readonly<ServiceProbeReport> | undefined { return this.#protocol.report(event); }
  commissioning(event: ServiceSetupEvent): Readonly<ServiceCommissioning> | undefined { return this.#protocol.commissioning(event); }
}
