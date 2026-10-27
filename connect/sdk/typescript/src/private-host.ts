/** Private local host protocol. The trusted launcher owns the child and endpoints.
 * This client neither authenticates source events nor admits native work. It must
 * never be connected to public RPC, HTTP, model callbacks or a remote socket.
 */
import type { Readable, Writable } from "node:stream";
import { createHash } from "node:crypto";
import { visit } from "jsonc-parser";
import { isObject, receiptJson, serviceBody, type JsonObject } from "./connectors/json.js";

export interface SelectedHostArtifact { readonly path: string; readonly sha256: string }
export interface HostBootstrapSelection {
  readonly deployment_root: string;
  readonly activation: SelectedHostArtifact;
  readonly policy_manifest: SelectedHostArtifact;
  readonly local_policy: SelectedHostArtifact;
}
/** The trusted application's selected schema-4 plan. This is not authority. */
export interface ApplicationPreparation { readonly plan: SelectedHostArtifact }
/** One private preparation and its one later adoption, never a replay handle. */
export interface ApplicationPreparationAttempt {
  readonly manifest: SelectedHostArtifact | null;
  /** Retained before the adoption write, even if acknowledgement is lost. */
  readonly selection: HostBootstrapSelection | null;
  observe(): Promise<SelectedHostArtifact>;
  adopt(selection: HostBootstrapSelection): Promise<void>;
}
/** Trusted launcher's existing admitted update. Output custody is not authority
 * or completion. Prior policy must be the actual accepted desired predecessor.
 */
export interface ApplicationUpdateStartup {
  readonly selection: SelectedHostArtifact;
  readonly prior_tool_policy: SelectedHostArtifact;
  readonly association_directory: string;
}
export interface PrivateSourceEvidence {
  readonly resource_id: string; readonly revision: string; readonly sha256: string;
}
export interface PrivateAuthenticatedEvent {
  readonly route: string;
  readonly original: string;
  readonly occurrence: string;
  readonly original_authentication: PrivateSourceEvidence;
  readonly root_event: PrivateSourceEvidence;
  readonly contributors: readonly string[];
  readonly content_sha256: string;
}
/** Historical source identity, never an admission or permission. */
export interface PrivateOriginalRef {
  readonly route: string; readonly original: string; readonly event_sha256: string;
}
export interface PrivateOriginalRetention {
  readonly original: PrivateOriginalRef; readonly state: "retained_pending_native_admission";
}
/** Source bookkeeping after native admission, separate from task completion. */
export type PrivateSourceSettlement = "fresh_acceptance_promoted" | "retry_withdrawn" | "post_admission_publication_failed";
/** Native admission observation. A turn ticket does not attest task completion. */
export type PrivateOriginalSubmission = {
  readonly state: "admitted";
  readonly session_id: string;
  readonly input_id: string;
  readonly deduplicated: boolean;
  readonly turn_ticket: string | null;
  readonly source_settlement: PrivateSourceSettlement;
} | { readonly state: "admission_unresolved" } | { readonly state: "native_outcome_unavailable" };
/** Durable cancellation of this original's operations, not settlement of a run, controller or descendants. */
export interface PrivateOriginalCancellation {
  readonly original: PrivateOriginalRef; readonly state: "operations_cancelled";
}
export type HostCloseReason = "shutdown" | "operator_disable" | "quarantine" | "activation_replaced" | "host_lost";
export type HostCloseScope = { readonly kind: "activation" } | { readonly kind: "component"; readonly component: string };
export interface HostCloseHandle {
  readonly incarnation: string; readonly close_id: string; readonly scope: HostCloseScope;
}
export interface HostCloseObservation {
  readonly handle: HostCloseHandle;
  readonly state: "closing" | "settled" | "uncertain" | "failed";
  readonly failure: null | "physical_settlement_uncertain" | "policy_unavailable" | "owner_failed";
}
export interface HostComponentObservation {
  readonly incarnation: string; readonly component: string;
  /** Exact safe integer. Larger native u64 values are refused, never rounded. */
  readonly revision: number;
  readonly state: "staged" | "open" | "closing" | "closed" | "quarantined" | "uncertain" | "failed";
}
export type HostRefusal = "source_conflict" | "invalid_selection" | "unavailable" | "denied" | "wrong_owner" | "not_settled";
export class HostControlError extends Error {}
/** The shared channel has failed or closed. An admitted effect may have occurred;
 * this error never authorizes replay or attests native settlement.
 */
export class HostTransportError extends HostControlError {}
/** An exact decoded native error reply, not proof that no effect occurred.
 * The reply itself does not close the channel. Concurrent transport loss or a
 * failed bootstrap can still close it: this client has not established an
 * observed usable bootstrap channel.
 */
export class HostRefusedError extends HostControlError {
  constructor(readonly refusal: HostRefusal) { super(`native host refused: ${refusal}`); }
}
/** Only this observation expired. Its late reply is still drained without resend.
 * An incomplete bootstrap separately closes its unusable channel.
 */
export class HostObservationTimeout extends HostControlError {
  constructor() { super("native host observation timed out; settlement is unknown"); }
}
/** Invocation correlation selected by the authenticated application, not an operator identity. */
export interface RemoteControllerLogin { readonly scope_id: string; readonly expires_at_ms: number }
export type ServiceReceiptScope = { readonly kind: "run"; readonly run_id: string }
  | { readonly kind: "runtime_termination" };
/** Native serialized payloads remain native-owned, including finalization failures. */
export type ServiceInputReport =
  | { readonly status: "pending"; readonly input_id: string; readonly phase: string;
      readonly terminal: JsonObject | null; readonly last_run_id: string | null; readonly attempt_count: number }
  | { readonly status: "terminal_without_receipt"; readonly input_id: string;
      readonly terminal: JsonObject; readonly last_run_id: string | null; readonly attempt_count: number }
  | { readonly status: "finalized"; readonly input_id: string; readonly terminal: JsonObject;
      readonly attempt_count: number; readonly scope: ServiceReceiptScope; readonly run_id: string | null;
      readonly owner_input_id: string; readonly recipient_input_ids: readonly string[]; readonly outcome: JsonObject };
export interface ServiceInputReceipt {
  readonly source: "live_runtime" | "durable_store";
  readonly report: ServiceInputReport;
}
/** Admission unresolved is not no-effect proof; a null receipt is a successful native read of absence. */
export type ServiceInputObservation = { readonly state: "admission_unresolved" }
  | { readonly state: "admitted"; readonly session_id: string; readonly input_id: string;
      readonly receipt: ServiceInputReceipt | null };
/** Native lineage data for the existing trusted compiler. This is not current permission. */
export type ControllerParentOutcome =
  | { readonly state: "denied" | "unavailable" | "pending_reconciliation" }
  | { readonly state: "selected"; readonly route: string; readonly purpose: "controller"; readonly lineage: readonly JsonObject[] };
/** Native target identity only. It conveys neither permission nor currentness. */
export type SetupGrantTarget =
  | { readonly kind: "mcp_server"; readonly server: string }
  | { readonly kind: "registered_tool_bundle_operation"; readonly bundle: string; readonly operation: string };
/** Independent native parent and restriction data, never a client-composed union. */
export interface SetupHelperTarget {
  readonly target: SetupGrantTarget | null;
  readonly parent: JsonObject;
  /** Native ExecutionRestrictions fields; their contents remain owner data. */
  readonly restrictions: JsonObject;
}
/** Exact pending-original setup selection. Native admission and service approvals are separate. */
export type SetupRequestOutcome =
  | { readonly state: "denied" | "unavailable" | "pending_reconciliation" }
  | { readonly state: "setup_request_selected"; readonly selection: {
      readonly original: PrivateOriginalRef; readonly session_id: string; readonly parent: readonly JsonObject[];
      readonly helper_targets?: readonly SetupHelperTarget[] } };
export interface RemoteSetupRequestReview {
  readonly reviewId: number;
  readonly outcome: Promise<SetupRequestOutcome>;
}
export type ControllerReviewStatus =
  | { readonly state: "awaiting_prompt" | "answer_forwarded" | "retired" }
  | { readonly state: "prompt"; readonly prompt: JsonObject };
export interface RemoteControllerReview {
  /** Only meaningful on this same private connection. */
  readonly reviewId: number;
  /** Native final outcome; forwarding an answer does not complete this promise. */
  readonly outcome: Promise<ControllerParentOutcome>;
}
/** Parent-selected labels. Only the native verified carrier authenticates them. */
export interface ConsoleLoginSelection {
  readonly issuer: string; readonly subject: string; readonly login_scope_id: string; readonly expires_at_ms: number;
}
/** A bounded local observation, not completed native/account cleanup. */
export interface ConsoleLoginRetirement {
  readonly fenced: true; readonly reservations_drained: true; readonly uncertain: boolean;
}
export interface ConsoleLoginEnrollment {
  readonly enrollmentId: number;
  /** Must succeed before the application mints a token for this enrollment. */
  readonly outcome: Promise<void>;
  /** Same original correlation, including after an unobserved enrollment answer. */
  retire(options?: { timeoutMs?: number }): Promise<ConsoleLoginRetirement>;
}
/** Implement in the application's actual credential/rule owner. These fields
 * do not authenticate an administrator. checkCurrent is synchronous and returns
 * normally only while the exact authorization is current; refusal uses the
 * typed HostRefusedError. The owner fences this exchange before invalidation.
 * release runs only after exact operation-owner Joined custody. Releasing a
 * standing reservation does not revoke the underlying standing rule.
 */
export interface ApplicationAdministrationReservation {
  readonly proposal_sha256: string;
  readonly authorization_ref: string;
  readonly provenance: JsonObject;
  checkCurrent(): void;
  release(): void;
}
/** Also represents a login-bound standing rule, with stable rule provenance.
 * The acknowledged same-client enrollment is lifetime only, never auth or R3.
 */
export interface CurrentLoginDecision {
  readonly kind: "current_login_decision";
  readonly reservation: ApplicationAdministrationReservation;
  readonly login: ConsoleLoginEnrollment;
}
/** Actual finite-rule authorization independent of a current human login. */
export interface StandingRuleDecision {
  readonly kind: "standing_rule_decision";
  readonly reservation: ApplicationAdministrationReservation;
}

export interface ServiceToolPolicySelection {
  readonly tools: readonly { readonly name: string; readonly consequence: "r0" | "r1" | "r2" | "r3" }[];
  readonly catalog: "r0" | "r1" | "r2" | "r3" | null;
}
export interface ServiceToolPolicyVersion { readonly revision: number; readonly digest: string }
export interface ServiceToolPolicyProposal {
  readonly provider_id: string; readonly policy_id: string; readonly member: string;
  readonly selection: ServiceToolPolicySelection; readonly catalog_session: string | null;
  readonly prior: ServiceToolPolicyVersion; readonly candidate: ServiceToolPolicyVersion;
  /** Exact complete native artifact, including its final newline and preserved grants. */
  readonly candidate_canonical_json: string;
  readonly lifetime: "until_superseded";
}
export interface ServiceToolPolicyObservation {
  readonly proposal_sha256: string; readonly candidate: ServiceToolPolicyVersion;
  readonly native: "accepted" | "refused" | null;
  /** Observed durable promotion, not a guarantee of future currentness. */
  readonly retained_as_current: boolean;
}
export interface ServiceParentAuthorizationProposal {
  readonly operator: JsonObject;
  readonly rule: { readonly kind: "application_policy_tasks"; readonly routes: readonly string[] };
  readonly issuance_id: string;
  readonly deployment: string;
  readonly source_policy_sha256: string;
  readonly original: PrivateOriginalRef;
  readonly acquisition: { readonly realm: string; readonly owner: string; readonly id: string };
  readonly instruction_sha256: string;
  readonly expires_in_ms: number | null;
  readonly application: JsonObject;
  readonly represented_subject: JsonObject | null;
  readonly decision: JsonObject;
  readonly connector_selection: JsonObject;
  readonly restrictions: JsonObject;
  readonly tool_policy?: ServiceToolPolicyProposal;
}
/** This is an operation-owner observation, never attachment or task completion. */
export interface ServiceParentOperationObservation {
  readonly outcome: "succeeded" | "refused_before_entry" | "failed_after_effects" | "uncertain";
  readonly custody: "joined";
  readonly reason: "requires_human" | "denied" | "unavailable" | "not_settled" | "conflict" | "cancelled" | null;
  readonly parent_issuance_id: string | null;
  readonly parent_issued: boolean;
  readonly child: { readonly grant: { readonly route: string; readonly decision: string; readonly decision_sha256: string };
    readonly state: "approval_denied" | "grant_issued" | "grant_unavailable" | "pending_reconciliation" } | null;
  readonly source: PrivateOriginalRef | null;
  readonly tool_policy?: ServiceToolPolicyObservation;
}
export type ServiceParentResult = ServiceParentOperationObservation | { readonly state: "not_submitted" };
export interface ServiceParentAdministrationAttempt {
  /** Explicit once-only forwarding. An ACK is not issuance or successful setup. */
  submit(options?: { timeoutMs?: number }): Promise<{ readonly state: "answer_forwarded" }>;
  result(options?: { timeoutMs?: number }): Promise<ServiceParentResult>;
  retire(options?: { timeoutMs?: number }): Promise<ServiceParentOperationObservation>;
}
export interface ServiceParentAdministrationRequest {
  readonly exchangeId: number;
  readonly proposalSha256: string;
  readonly toolPolicySha256: string | null;
  readonly proposal: ServiceParentAuthorizationProposal;
  readonly attempt: ServiceParentAdministrationAttempt | null;
  /** Synchronous local transfer. No answer write or new task is started here. */
  authorize(decision: CurrentLoginDecision | StandingRuleDecision): ServiceParentAdministrationAttempt;
  /** The actual held approval covers the COMPLETE candidate until superseded,
   * including preserved permissions. Catalog lets the waiting original agent
   * find tools in the original conversation; service/catalog grants stay task-scoped.
   * No new prompt or write occurs. */
  authorizeToolPolicy(decision: CurrentLoginDecision | StandingRuleDecision, toolPolicySha256: string): ServiceParentAdministrationAttempt;
  requiresHuman(options?: { timeoutMs?: number }): Promise<{ readonly state: "answer_forwarded" }>;
  deny(options?: { timeoutMs?: number }): Promise<{ readonly state: "answer_forwarded" }>;
  unavailable(options?: { timeoutMs?: number }): Promise<{ readonly state: "answer_forwarded" }>;
  result(options?: { timeoutMs?: number }): Promise<ServiceParentResult>;
  retire(options?: { timeoutMs?: number }): Promise<ServiceParentOperationObservation>;
}

const consoleLoginCustody = Symbol("toolkit-console-login-custody");
interface ConsoleLoginCustody {
  readonly client: PrivateHostClient;
  readonly enrollment: ConsoleLoginEnrollment;
  current(): boolean;
}
type RetainedConsoleLogin = ConsoleLoginEnrollment & { readonly [consoleLoginCustody]: ConsoleLoginCustody };
interface ServiceParentWireRequest {
  readonly exchange_id: number;
  readonly proposal_sha256: string;
  readonly proposal: ServiceParentAuthorizationProposal;
}
function servicePolicyDigest(value: unknown): string {
  if (typeof value !== "string" || !value.startsWith("sha256:")) invalid();
  digest(value.slice(7));
  return value;
}
function serviceToolPolicy(value: unknown): ServiceToolPolicyProposal {
  const row = fields(value, ["provider_id", "policy_id", "member", "selection", "catalog_session",
    "prior", "candidate", "candidate_canonical_json", "lifetime"]);
  for (const key of ["provider_id", "policy_id", "member"]) identifier(row[key]);
  const selection = fields(row.selection, ["tools", "catalog"]);
  if (!Array.isArray(selection.tools) || selection.tools.length > 128) invalid();
  const catalog = selection.catalog === null ? null : oneOf(selection.catalog, ["r0", "r1", "r2", "r3"]);
  const names = new Set<string>();
  for (const value of selection.tools) {
    const tool = fields(value, ["name", "consequence"]);
    const name = text(tool.name);
    if (!name || name.trim() !== name || encoder.encode(name).length > 512 || /\p{Cc}/u.test(name) || names.has(name)) invalid();
    oneOf(tool.consequence, ["r0", "r1", "r2", "r3"]); names.add(name);
  }
  if (catalog !== null) {
    if (names.has("tool_catalog_search") || names.has("tool_catalog_load")) invalid();
    identifier(row.catalog_session);
  } else if (row.catalog_session !== null) invalid();
  if (!names.size && catalog === null) invalid();
  for (const field of ["prior", "candidate"]) {
    const version = fields(row[field], ["revision", "digest"]);
    // Native transparent u64 has a wider domain; this channel refuses values
    // outside its safe-integer profile rather than rounding them.
    if (typeof version.revision !== "number" || !Number.isSafeInteger(version.revision) || version.revision < 0) invalid();
    servicePolicyDigest(version.digest);
  }
  const canonical = text(row.candidate_canonical_json);
  if (row.lifetime !== "until_superseded" || !canonical.endsWith("\n")
    || "sha256:" + createHash("sha256").update(canonical).digest("hex") !== (row.candidate as JsonObject).digest) invalid();
  return snapshot(row) as unknown as ServiceToolPolicyProposal;
}
function serviceToolPolicyDigest(value: ServiceToolPolicyProposal): string {
  const proposal = serviceToolPolicy(value);
  function sorted(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(sorted);
    if (isObject(value)) return Object.fromEntries(Object.keys(value).sort().map(key => [key, sorted(value[key])]));
    return value;
  }
  // Native canonical_json sorts these closed ASCII keys. The native artifact
  // stays an exact string; safe revisions cannot round during JSON encoding.
  return createHash("sha256").update(JSON.stringify(sorted(proposal))).digest("hex");
}

function serviceParentRequest(reply: JsonObject): ServiceParentWireRequest {
  fields(reply, ["kind", "exchange_id", "proposal_sha256", "proposal"]);
  if (reply.kind !== "service_parent_administration_request") invalid();
  const row = fields(reply.proposal, ["operator", "rule", "issuance_id", "deployment",
    "source_policy_sha256", "original", "acquisition", "instruction_sha256", "expires_in_ms",
    "application", "represented_subject", "decision", "connector_selection", "restrictions",
    ...(isObject(reply.proposal) && "tool_policy" in reply.proposal ? ["tool_policy"] : [])]);
  if ("tool_policy" in row) serviceToolPolicy(row.tool_policy);
  for (const key of ["operator", "application", "decision", "connector_selection", "restrictions"] as const) {
    if (!isObject(row[key])) invalid();
  }
  if (row.represented_subject !== null && !isObject(row.represented_subject)) invalid();
  identifier(row.issuance_id); id(row.deployment); digest(row.source_policy_sha256); digest(row.instruction_sha256);
  original(row.original);
  const acquisition = fields(row.acquisition, ["realm", "owner", "id"]);
  Object.values(acquisition).forEach(identifier);
  if (row.expires_in_ms !== null && (typeof row.expires_in_ms !== "number" || !Number.isSafeInteger(row.expires_in_ms) || row.expires_in_ms < 0)) invalid();
  const rule = fields(row.rule, ["kind", "routes"]);
  if (rule.kind !== "application_policy_tasks" || !Array.isArray(rule.routes) || !rule.routes.length) invalid();
  const routes = rule.routes.map(id);
  if (new Set(routes).size !== routes.length) invalid();
  return { exchange_id: positiveInteger(reply.exchange_id), proposal_sha256: digest(reply.proposal_sha256),
    proposal: snapshot(row) as unknown as ServiceParentAuthorizationProposal };
}
function serviceParentObservation(value: unknown, selected: ServiceParentWireRequest): ServiceParentOperationObservation {
  const row = fields(value, ["outcome", "custody", "reason", "parent_issuance_id", "parent_issued", "child", "source",
    ...(isObject(value) && "tool_policy" in value ? ["tool_policy"] : [])]);
  let policy: ServiceToolPolicyObservation | undefined;
  if ("tool_policy" in row) {
    const observed = fields(row.tool_policy, ["proposal_sha256", "candidate", "native", "retained_as_current"]);
    const proposal = selected.proposal.tool_policy;
    const candidate = fields(observed.candidate, ["revision", "digest"]);
    if (!proposal || digest(observed.proposal_sha256) !== serviceToolPolicyDigest(proposal)
      || candidate.revision !== proposal.candidate.revision || servicePolicyDigest(candidate.digest) !== proposal.candidate.digest
      || typeof observed.retained_as_current !== "boolean") invalid();
    const native = observed.native === null ? null : oneOf(observed.native, ["accepted", "refused"]);
    if (observed.retained_as_current && native !== "accepted") invalid();
    policy = { proposal_sha256: digest(observed.proposal_sha256), candidate: proposal.candidate,
      native, retained_as_current: observed.retained_as_current };
  }
  const outcome = oneOf(row.outcome, ["succeeded", "refused_before_entry", "failed_after_effects", "uncertain"]);
  if (row.custody !== "joined" || typeof row.parent_issued !== "boolean") invalid();
  const reason = row.reason === null ? null : oneOf(row.reason, ["requires_human", "denied", "unavailable", "not_settled", "conflict", "cancelled"]);
  const parent = row.parent_issuance_id === null ? null : identifier(row.parent_issuance_id);
  if (parent !== null && parent !== selected.proposal.issuance_id) invalid();
  let child: ServiceParentOperationObservation["child"] = null;
  if (row.child !== null) {
    const selectedChild = fields(row.child, ["grant", "state"]);
    const grant = fields(selectedChild.grant, ["route", "decision", "decision_sha256"]);
    child = { grant: { route: id(grant.route), decision: id(grant.decision), decision_sha256: digest(grant.decision_sha256) },
      state: oneOf(selectedChild.state, ["approval_denied", "grant_issued", "grant_unavailable", "pending_reconciliation"]) };
  }
  return { outcome, custody: "joined", reason, parent_issuance_id: parent, parent_issued: row.parent_issued,
    child, source: row.source === null ? null : original(row.source), ...(policy ? { tool_policy: policy } : {}) };
}
function observeServiceParent<T>(result: Promise<T>, duration: number): Promise<T> {
  timeout(duration);
  // This timer detaches only the observer. The client/exchange still owns the
  // original call, guard and eventual response, including a late Joined result.
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new HostObservationTimeout()), duration);
    void result.then(value => { clearTimeout(timer); resolve(value); }, error => { clearTimeout(timer); reject(error); });
  });
}
type ServiceParentStart = <T>(command: object, decode: (reply: JsonObject) => T,
  retirement?: boolean) => { id: number; result: Promise<T> };

/** One bounded connection-owned exchange, independent of disposable waiters. */
class ServiceParentExchange implements ServiceParentAdministrationRequest {
  readonly #client: PrivateHostClient;
  readonly #start: ServiceParentStart;
  readonly #duration: number;
  readonly #releaseSlot: () => void;
  #selected: ServiceParentWireRequest | null = null;
  #next: Promise<ServiceParentWireRequest> | null = null;
  // Original correlations outlive disposable pending response callbacks.
  #nextCorrelation: number | null = null;
  #answerCorrelation: number | null = null;
  #resultCorrelation: number | null = null;
  #retirementCorrelation: number | null = null;
  #authorization: { guard: ApplicationAdministrationReservation | null;
    readonly login: RetainedConsoleLogin | null; readonly attempt: ServiceParentAdministrationAttempt;
    readonly decision: JsonObject } | null = null;
  #negative: JsonObject | null = null;
  #submission: Promise<{ readonly state: "answer_forwarded" }> | null = null;
  #submitStarted = false;
  #outcome: Promise<ServiceParentOperationObservation> | null = null;
  #retirement: Promise<ServiceParentOperationObservation> | null = null;
  #fenced = false;
  #joined: ServiceParentOperationObservation | null = null;
  #releaseStarted = false;
  #releaseError: unknown;
  #releaseFailed = false;

  constructor(client: PrivateHostClient, start: ServiceParentStart, duration: number, releaseSlot: () => void) {
    this.#client = client; this.#start = start; this.#duration = duration; this.#releaseSlot = releaseSlot;
  }
  #request(): ServiceParentWireRequest {
    if (this.#selected === null) throw new HostControlError("service-parent proposal has not been observed");
    return this.#selected;
  }
  get exchangeId(): number { return this.#request().exchange_id; }
  get proposalSha256(): string { return this.#request().proposal_sha256; }
  get toolPolicySha256(): string | null {
    const proposal = this.#request().proposal.tool_policy;
    return proposal ? serviceToolPolicyDigest(proposal) : null;
  }
  get proposal(): ServiceParentAuthorizationProposal { return structuredClone(this.#request().proposal); }
  get attempt(): ServiceParentAdministrationAttempt | null { return this.#authorization?.attempt ?? null; }

  async observeRequest(options: { timeoutMs?: number }): Promise<ServiceParentAdministrationRequest> {
    const duration = timeout(options.timeoutMs ?? this.#duration);
    if (this.#next === null) {
      try {
        if (this.#nextCorrelation !== null) throw new HostControlError("original service-parent correlation is already retained");
        const started = this.#start({ kind: "next_service_parent_administration" }, serviceParentRequest);
        this.#nextCorrelation = started.id;
        this.#next = started.result.then(value => { this.#selected = value; return value; });
      } catch (error) { this.#next = Promise.reject(error); }
      void this.#next.catch(() => {});
    }
    try {
      await observeServiceParent(this.#next, duration);
    } catch (error) {
      // Only a decoded refusal ends an unanswered query. A timeout or lost
      // transport keeps this same correlation and all selected custody.
      if (error instanceof HostRefusedError && this.#selected === null
        && this.#authorization === null && this.#negative === null && !this.#submitStarted) {
        this.#nextCorrelation = null;
        this.#releaseSlot();
      }
      throw error;
    }
    return this;
  }
  authorize(value: CurrentLoginDecision | StandingRuleDecision): ServiceParentAdministrationAttempt {
    return this.#authorize(value, null);
  }
  authorizeToolPolicy(value: CurrentLoginDecision | StandingRuleDecision, toolPolicySha256: string): ServiceParentAdministrationAttempt {
    return this.#authorize(value, digest(toolPolicySha256));
  }
  #authorize(value: CurrentLoginDecision | StandingRuleDecision, toolPolicySha256: string | null): ServiceParentAdministrationAttempt {
    const selected = this.#request();
    if (this.#fenced || this.#authorization !== null || this.#negative !== null) {
      throw new HostControlError("service-parent exchange is already answered or retired");
    }
    this.#client.assertReady();
    if (toolPolicySha256 !== this.toolPolicySha256) throw new HostControlError("the complete tool policy requires its exact explicit approval");
    if (!value || !["current_login_decision", "standing_rule_decision"].includes(value.kind)) invalid();
    const guard = value.reservation;
    if (!guard || typeof guard.checkCurrent !== "function" || typeof guard.release !== "function"
      || digest(guard.proposal_sha256) !== selected.proposal_sha256) invalid();
    const reference = identifier(guard.authorization_ref);
    const provenance = snapshot(guard.provenance);
    if (!Object.keys(provenance).length || encoder.encode(serviceBody(provenance)).length > 12 * 1024) invalid();
    let login: RetainedConsoleLogin | null = null;
    let source: JsonObject = { kind: "standing_rule_decision" };
    if (value.kind === "current_login_decision") {
      login = value.login as RetainedConsoleLogin;
      if (!login || login[consoleLoginCustody]?.client !== this.#client || login[consoleLoginCustody].enrollment !== login || !login[consoleLoginCustody].current()) {
        throw new HostControlError("administration needs the acknowledged same-client login");
      }
      source = { kind: "current_login_decision", enrollment_id: login.enrollmentId };
    }
    if (guard.checkCurrent() !== undefined) throw new HostControlError("administration currentness must be synchronous");
    const attempt = Object.freeze({
      submit: (options: { timeoutMs?: number } = {}) => this.#submit(options),
      result: (options: { timeoutMs?: number } = {}) => this.result(options),
      retire: (options: { timeoutMs?: number } = {}) => this.retire(options),
    });
    // One synchronous assignment owns the actual guard and publishes its alias
    // before returning. No response callback or promise exclusively owns it.
    this.#authorization = { guard, login, attempt,
      decision: { kind: toolPolicySha256 === null ? "authorized" : "authorized_with_tool_policy",
        source, authorization_ref: reference, provenance,
        ...(toolPolicySha256 === null ? {} : { tool_policy_sha256: toolPolicySha256 }) } };
    return attempt;
  }
  #negativeAnswer(kind: "requires_human" | "denied" | "unavailable", options: { timeoutMs?: number }): Promise<{ readonly state: "answer_forwarded" }> {
    if (this.#fenced || this.#authorization !== null) return Promise.reject(new HostControlError("service-parent exchange is authorized or retired"));
    if (this.#negative !== null && this.#negative.kind !== kind) return Promise.reject(new HostControlError("service-parent exchange already retained another answer"));
    this.#negative ??= { kind };
    return this.#submit(options);
  }
  requiresHuman(options: { timeoutMs?: number } = {}): Promise<{ readonly state: "answer_forwarded" }> { return this.#negativeAnswer("requires_human", options); }
  deny(options: { timeoutMs?: number } = {}): Promise<{ readonly state: "answer_forwarded" }> { return this.#negativeAnswer("denied", options); }
  unavailable(options: { timeoutMs?: number } = {}): Promise<{ readonly state: "answer_forwarded" }> { return this.#negativeAnswer("unavailable", options); }

  #submit(options: { timeoutMs?: number }): Promise<{ readonly state: "answer_forwarded" }> {
    const duration = timeout(options.timeoutMs ?? this.#duration);
    if (this.#submission === null) {
      try {
        if (this.#fenced) throw new HostControlError("service-parent exchange is retired before submission");
        const decision = this.#authorization?.decision ?? this.#negative;
        if (decision === null) throw new HostControlError("service-parent decision has not been retained");
        const guard = this.#authorization?.guard;
        if (guard) {
          if (this.#authorization?.login && !this.#authorization.login[consoleLoginCustody].current()) throw new HostRefusedError("denied");
          if (guard.checkCurrent() !== undefined) throw new HostControlError("administration currentness must be synchronous");
        }
        // Single-threaded synchronous scheduling linearizes against retire().
        // Any subsequent queued/partial write remains potentially entered.
        this.#submitStarted = true;
        if (this.#answerCorrelation !== null) throw new HostControlError("original service-parent correlation is already retained");
        const started = this.#start({ kind: "submit_service_parent_administration",
          exchange_id: this.exchangeId, proposal_sha256: this.proposalSha256, decision }, reply => {
          fields(reply, ["kind", "exchange_id"]);
          if (reply.kind !== "service_parent_administration_submitted" || positiveInteger(reply.exchange_id) !== this.exchangeId) invalid();
          return { state: "answer_forwarded" as const };
        });
        this.#answerCorrelation = started.id;
        this.#submission = started.result;
      } catch (error) {
        this.#fenced = true;
        this.#submission = Promise.reject(error);
      }
      void this.#submission.catch(() => {});
    }
    return observeServiceParent(this.#submission, duration);
  }
  #acceptJoined(value: ServiceParentOperationObservation): ServiceParentOperationObservation {
    if (this.#joined !== null && serviceBody(this.#joined) !== serviceBody(value)) invalid();
    this.#joined = value;
    if (!this.#releaseStarted) {
      this.#releaseStarted = true;
      const guard = this.#authorization?.guard;
      if (guard) {
        try {
          if (guard.release() !== undefined) throw new HostControlError("administration release must be synchronous");
          this.#authorization!.guard = null;
        } catch (error) { this.#releaseError = error; this.#releaseFailed = true; }
      }
    }
    if (this.#releaseFailed) throw this.#releaseError;
    return structuredClone(value);
  }
  #observation(reply: JsonObject, kind: string): ServiceParentOperationObservation {
    fields(reply, ["kind", "exchange_id", "observation"]);
    if (reply.kind !== kind || positiveInteger(reply.exchange_id) !== this.exchangeId) invalid();
    return serviceParentObservation(reply.observation, this.#request());
  }
  result(options: { timeoutMs?: number } = {}): Promise<ServiceParentResult> {
    const duration = timeout(options.timeoutMs ?? this.#duration);
    if (this.#joined !== null) {
      try { return Promise.resolve(this.#acceptJoined(this.#joined)); }
      catch (error) { return Promise.reject(error); }
    }
    if (!this.#submitStarted) return Promise.resolve({ state: "not_submitted" });
    if (this.#outcome === null) {
      try {
        if (this.#resultCorrelation !== null) throw new HostControlError("original service-parent correlation is already retained");
        const started = this.#start({ kind: "result_service_parent_administration", exchange_id: this.exchangeId },
          reply => this.#observation(reply, "service_parent_administration_result"));
        this.#resultCorrelation = started.id;
        this.#outcome = started.result.then(value => this.#acceptJoined(value));
      } catch (error) { this.#outcome = Promise.reject(error); }
      void this.#outcome.catch(() => {});
    }
    return observeServiceParent(this.#outcome, duration).then(value => structuredClone(value));
  }
  retire(options: { timeoutMs?: number } = {}): Promise<ServiceParentOperationObservation> {
    const duration = timeout(options.timeoutMs ?? this.#duration);
    this.#fenced = true;
    if (this.#retirement === null) {
      try {
        if (this.#retirementCorrelation !== null) throw new HostControlError("original service-parent correlation is already retained");
        const started = this.#start({ kind: "retire_service_parent_administration", exchange_id: this.exchangeId },
          reply => this.#observation(reply, "service_parent_administration_retired"), true);
        this.#retirementCorrelation = started.id;
        this.#retirement = started.result.then(value => {
          const observed = this.#acceptJoined(value);
          this.#releaseSlot();
          return observed;
        });
      } catch (error) { this.#retirement = Promise.reject(error); }
      void this.#retirement.catch(() => {});
    }
    return observeServiceParent(this.#retirement, duration).then(value => structuredClone(value));
  }
}

const MAX_FRAME = 1_048_576;
const MAX_PENDING = 32;
const encoder = new TextEncoder();
const refusals: readonly HostRefusal[] = ["source_conflict", "invalid_selection", "unavailable", "denied", "wrong_owner", "not_settled"];
function invalid(): never { throw new HostControlError("private host data was rejected"); }
function fields(value: unknown, keys: readonly string[]): JsonObject {
  if (!isObject(value) || Object.keys(value).length !== keys.length || keys.some(key => !Object.hasOwn(value, key))) invalid();
  return value;
}
function text(value: unknown): string {
  if (typeof value !== "string" || /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/u.test(value)) invalid();
  return value;
}
function identifier(value: unknown): string {
  const result = text(value);
  if (!result || /^\p{White_Space}*$/u.test(result) || encoder.encode(result).length > 256 || /\p{Cc}/u.test(result)) invalid();
  return result;
}
function positiveInteger(value: unknown): number {
  if (typeof value !== "number" || !Number.isSafeInteger(value) || value <= 0) invalid();
  return value;
}
function serviceInputObservation(value: unknown): ServiceInputObservation {
  if (isObject(value) && value.state === "admission_unresolved") {
    fields(value, ["state"]); return { state: "admission_unresolved" };
  }
  const observed = fields(value, ["state", "session_id", "input_id", "receipt"]);
  if (observed.state !== "admitted") invalid();
  identifier(observed.session_id);
  const selected = identifier(observed.input_id);
  if (observed.receipt === null) return observed as unknown as ServiceInputObservation;
  const receipt = fields(observed.receipt, ["source", "report"]);
  oneOf(receipt.source, ["live_runtime", "durable_store"]);
  if (!isObject(receipt.report)) invalid();
  const report = receipt.report;
  const common = ["status", "input_id", "terminal", "attempt_count"];
  if (report.status === "pending") {
    fields(report, [...common, "phase", "last_run_id"]);
    oneOf(report.phase, ["accepted", "queued", "staged", "applied", "applied_pending_consumption", "consumed", "superseded", "coalesced", "abandoned"]);
  } else if (report.status === "terminal_without_receipt") {
    fields(report, [...common, "last_run_id"]);
  } else if (report.status === "finalized") {
    fields(report, [...common, "scope", "run_id", "owner_input_id", "recipient_input_ids", "outcome"]);
    if (isObject(report.scope) && report.scope.kind === "run") {
      const scope = fields(report.scope, ["kind", "run_id"]);
      if (identifier(scope.run_id) !== report.run_id) invalid();
    } else {
      const scope = fields(report.scope, ["kind"]);
      if (scope.kind !== "runtime_termination" || report.run_id !== null) invalid();
    }
    const owner = identifier(report.owner_input_id);
    if (!Array.isArray(report.recipient_input_ids) || report.recipient_input_ids.length === 0) invalid();
    const recipients = report.recipient_input_ids.map(identifier);
    if (recipients[0] !== owner || !recipients.includes(selected) || new Set(recipients).size !== recipients.length) invalid();
    if (!isObject(report.outcome)) invalid();
    oneOf(report.outcome.completion_type, ["completed", "completed_without_result", "callback_pending", "callback_batch_pending",
      "cancelled", "abandoned", "abandoned_with_error", "completed_with_finalization_failure", "runtime_terminated"]);
  } else invalid();
  if (identifier(report.input_id) !== selected || typeof report.attempt_count !== "number"
    || !Number.isInteger(report.attempt_count) || report.attempt_count < 0 || report.attempt_count > 0xffff_ffff) invalid();
  if (report.status !== "finalized" && report.last_run_id !== null) identifier(report.last_run_id);
  if (report.terminal === null) {
    if (report.status !== "pending") invalid();
  } else {
    if (!isObject(report.terminal)) invalid();
    oneOf(report.terminal.outcome_type, ["consumed", "superseded", "coalesced", "abandoned"]);
  }
  // The exact native state/outcome objects survive; the SDK adds no success flag.
  return observed as unknown as ServiceInputObservation;
}
function nativeLineage(value: unknown): JsonObject[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > 64) invalid();
  return value.map(entry => {
      const ref = fields(entry, ["root_authority", "authority_namespace", "authority_generation", "authority_incarnation", "grant_id", "issued_revision"]);
      if (!isObject(ref.root_authority)) invalid();
      identifier(ref.authority_namespace); positiveInteger(ref.authority_generation);
      uuid(ref.authority_incarnation); identifier(ref.grant_id); positiveInteger(ref.issued_revision);
      return ref;
  });
}
function setupHelperTargets(value: unknown): SetupHelperTarget[] {
  if (!Array.isArray(value) || value.length >= 64) invalid();
  const seen = new Set<string>();
  return value.map(item => {
    const row = fields(item, ["target", "parent", "restrictions"]);
    let target: SetupGrantTarget | null = null;
    if (row.target !== null) {
      if (!isObject(row.target)) invalid();
      if (row.target.kind === "mcp_server") {
        const selected = fields(row.target, ["kind", "server"]);
        target = { kind: "mcp_server", server: text(selected.server) };
        if (!target.server) invalid();
      } else if (row.target.kind === "registered_tool_bundle_operation") {
        const selected = fields(row.target, ["kind", "bundle", "operation"]);
        target = { kind: "registered_tool_bundle_operation", bundle: text(selected.bundle), operation: text(selected.operation) };
        if (!target.bundle || !target.operation) invalid();
      } else invalid();
    }
    const key = JSON.stringify(target);
    if (seen.has(key)) invalid();
    seen.add(key);
    const parent = nativeLineage([row.parent])[0]!;
    const restrictions = fields(row.restrictions, [
      "actions", "resource_domains", "processors", "audiences", "lifetime", "delegation_depth",
    ]);
    if (!Object.values(restrictions).every(isObject)) invalid();
    // Preserve the individual bounds; only native owners evaluate their meaning.
    return { target, parent, restrictions };
  });
}
function controllerOutcome(value: unknown, route: string): ControllerParentOutcome {
  if (!isObject(value)) invalid();
  if (value.state === "selected") {
    fields(value, ["state", "route", "purpose", "lineage"]);
    if (id(value.route) !== route || value.purpose !== "controller" || !Array.isArray(value.lineage)
      || value.lineage.length === 0 || value.lineage.length > 64) invalid();
    const lineage = nativeLineage(value.lineage);
    return { state: "selected", route, purpose: "controller", lineage };
  }
  fields(value, ["state"]);
  return { state: oneOf(value.state, ["denied", "unavailable", "pending_reconciliation"]) };
}
function controllerStatus(value: unknown, allowed: readonly string[]): ControllerReviewStatus {
  if (!isObject(value) || typeof value.state !== "string" || !allowed.includes(value.state)) invalid();
  if (value.state === "prompt") {
    fields(value, ["state", "prompt"]);
    if (!isObject(value.prompt)) invalid();
    return { state: "prompt", prompt: value.prompt };
  }
  fields(value, ["state"]);
  return { state: oneOf(value.state, ["awaiting_prompt", "answer_forwarded", "retired"]) };
}
function id(value: unknown): string {
  const result = text(value);
  if (!/^[A-Za-z0-9][A-Za-z0-9._\-/:@]{0,191}$/.test(result) || result.split("/").some(part => !part || part === "." || part === "..")) invalid();
  return result;
}
function digest(value: unknown): string { const result = text(value); if (!/^[0-9a-f]{64}$/.test(result)) invalid(); return result; }
function uuid(value: unknown): string {
  const result = text(value);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(result)) invalid();
  return result;
}
function oneOf<T extends string>(value: unknown, choices: readonly T[]): T {
  if (typeof value !== "string" || !choices.includes(value as T)) invalid();
  return value as T;
}
function snapshot(value: unknown): JsonObject {
  try { const result = receiptJson(serviceBody(value)); if (!isObject(result)) invalid(); return result; }
  catch { return invalid(); }
}
function privateFrame(line: string): unknown {
  const parsed = receiptJson(line);
  const nativeOutcome = isObject(parsed) && isObject(parsed.result) && parsed.result.kind === "service_input_receipt";
  // Protocol counters remain unsigned integer tokens, including 1 versus 1.0.
  // Only this new reply's native CompletionOutcome subtree accepts the existing
  // finite JSON numeric profile. Its arbitrary callback/result data is not a
  // private control counter and must not be silently rewritten or discarded.
  const stack: { path: string[]; property: string }[] = [];
  const enter = (): void => {
    const parent = stack.at(-1);
    stack.push({ path: parent ? [...parent.path, parent.property] : [], property: "" });
  };
  const prefix = ["result", "observation", "receipt", "report", "outcome"];
  visit(line, {
    onObjectBegin: enter, onArrayBegin: enter,
    onObjectEnd: () => { stack.pop(); }, onArrayEnd: () => { stack.pop(); },
    onObjectProperty: property => { const frame = stack.at(-1); if (frame) frame.property = property; },
    onLiteralValue: (value: unknown, offset, length) => {
      const frame = stack.at(-1);
      const path = frame ? [...frame.path, frame.property] : [];
      const payload = nativeOutcome && prefix.every((part, index) => path[index] === part);
      if (!payload && typeof value === "number" && !/^(0|[1-9][0-9]*)$/.test(line.slice(offset, offset + length))) invalid();
    },
  });
  return parsed;
}
function evidence(value: unknown): PrivateSourceEvidence {
  const object = fields(value, ["resource_id", "revision", "sha256"]);
  return { resource_id: identifier(object.resource_id), revision: identifier(object.revision), sha256: digest(object.sha256) };
}
function eventData(value: unknown): PrivateAuthenticatedEvent {
  const object = fields(snapshot(value), ["route", "original", "occurrence", "original_authentication", "root_event", "contributors", "content_sha256"]);
  if (!Array.isArray(object.contributors) || object.contributors.length > 64) invalid();
  return { route: id(object.route), original: identifier(object.original), occurrence: identifier(object.occurrence),
    original_authentication: evidence(object.original_authentication), root_event: evidence(object.root_event),
    contributors: object.contributors.map(identifier), content_sha256: digest(object.content_sha256) };
}
function original(value: unknown): PrivateOriginalRef {
  const object = fields(value, ["route", "original", "event_sha256"]);
  return { route: id(object.route), original: identifier(object.original), event_sha256: digest(object.event_sha256) };
}
function sameOriginal(left: PrivateOriginalRef, right: PrivateOriginalRef): boolean {
  return left.route === right.route && left.original === right.original && left.event_sha256 === right.event_sha256;
}
function scope(value: unknown): HostCloseScope {
  if (!isObject(value)) invalid();
  if (value.kind === "activation") { fields(value, ["kind"]); return { kind: "activation" }; }
  fields(value, ["kind", "component"]);
  if (value.kind !== "component") invalid();
  return { kind: "component", component: id(value.component) };
}
function handle(value: unknown): HostCloseHandle {
  const object = fields(value, ["incarnation", "close_id", "scope"]);
  return { incarnation: uuid(object.incarnation), close_id: uuid(object.close_id), scope: scope(object.scope) };
}
function sameScope(left: HostCloseScope, right: HostCloseScope): boolean {
  return left.kind === "activation" ? right.kind === "activation" : right.kind === "component" && left.component === right.component;
}
function closeObservation(value: unknown): HostCloseObservation {
  const object = fields(value, ["handle", "state", "failure"]);
  return { handle: handle(object.handle), state: oneOf(object.state, ["closing", "settled", "uncertain", "failed"]),
    failure: object.failure === null ? null : oneOf(object.failure, ["physical_settlement_uncertain", "policy_unavailable", "owner_failed"] as const) };
}
function selectionData(value: unknown): HostBootstrapSelection {
  const object = fields(snapshot(value), ["deployment_root", "activation", "policy_manifest", "local_policy"]);
  function path(value: unknown): string { const result = text(value); if (!result || result.includes("\0")) invalid(); return result; }
  function artifact(value: unknown): SelectedHostArtifact {
    const object = fields(value, ["path", "sha256"]); return { path: path(object.path), sha256: digest(object.sha256) };
  }
  return { deployment_root: path(object.deployment_root), activation: artifact(object.activation),
    policy_manifest: artifact(object.policy_manifest), local_policy: artifact(object.local_policy) };
}
function updateData(value: unknown): ApplicationUpdateStartup {
  const row = fields(snapshot(value), ["selection", "prior_tool_policy", "association_directory"]);
  function path(value: unknown): string {
    const result = text(value); if (!result.startsWith("/") || result.includes("\0")) invalid(); return result;
  }
  function artifact(value: unknown): SelectedHostArtifact {
    const pin = fields(value, ["path", "sha256"]); return { path: path(pin.path), sha256: digest(pin.sha256) };
  }
  return { selection: artifact(row.selection), prior_tool_policy: artifact(row.prior_tool_policy),
    association_directory: path(row.association_directory) };
}
function preparationArtifact(value: unknown): SelectedHostArtifact {
  const row = fields(value, ["path", "sha256"]);
  const path = text(row.path);
  if (!path.startsWith("/") || path.includes("\0")) invalid();
  return Object.freeze({ path, sha256: digest(row.sha256) });
}
function preparationData(value: unknown): ApplicationPreparation {
  const row = fields(snapshot(value), ["plan"]);
  return Object.freeze({ plan: preparationArtifact(row.plan) });
}
// This closed event schema contains only strings and arrays. Sorting its ASCII
// object keys reproduces toolkit_package::canonical_json without number codecs.
function canonicalEvent(event: PrivateAuthenticatedEvent): string {
  function sorted(value: unknown): unknown {
    if (Array.isArray(value)) return value.map(sorted);
    if (isObject(value)) return Object.fromEntries(Object.keys(value).sort().map(key => [key, sorted(value[key])]));
    return value;
  }
  return JSON.stringify(sorted(event));
}
function timeout(value: number): number {
  if (!Number.isFinite(value) || value <= 0 || value > 2_147_483_647) throw new RangeError("private host timeout must fit a positive finite timer");
  return value;
}
interface Pending {
  readonly decode: (reply: JsonObject) => unknown;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
  writeTimer: ReturnType<typeof setTimeout>;
  abandoned: boolean;
  phase: "queued" | "writing" | "awaiting";
}

/** The launcher supplies exclusively owned local streams and closes their real
 * descriptors when canceled/aborted. This client does not spawn or reap a child.
 * Keep it alive while the Meta runtime performs its ordinary shutdown handshake.
 * close() joins this client's readers only; it never attests native settlement.
 */
export class PrivateHostClient {
  readonly #writer: Writable;
  readonly #reader: Readable;
  readonly #readerTask: Promise<void>;
  readonly #timeout: number;
  readonly #pending = new Map<number, Pending>();
  #writeTail: Promise<void> = Promise.resolve();
  #abortWrite: ((error: HostTransportError) => void) | null = null;
  #failure: HostTransportError | null = null;
  #nextId = 1;
  #serviceExchange: ServiceParentExchange | null = null;
  #state: "new" | "preparing" | "bootstrapping" | "ready" = "new";

  constructor(commands: Writable, responses: Readable, options: { timeoutMs?: number } = {}) {
    // Per-call timeouts may extend observation, never this transport budget.
    this.#timeout = timeout(options.timeoutMs ?? 60_000);
    if (commands.destroyed || responses.destroyed || commands.writableEnded || responses.readableEnded) {
      throw new HostTransportError("private host endpoints are unavailable");
    }
    if (responses.readableEncoding !== null) throw new HostControlError("private host endpoints are unavailable");
    this.#writer = commands;
    this.#reader = responses;
    commands.on("error", () => this.#fail(new HostTransportError("private host command stream failed; settlement is unknown")));
    this.#readerTask = this.#read();
  }

  #fail(error: HostTransportError): void {
    if (this.#failure !== null) return;
    this.#failure = error;
    for (const pending of this.#pending.values()) { clearTimeout(pending.timer); clearTimeout(pending.writeTimer); pending.reject(error); }
    this.#pending.clear();
    // Destruction need not invoke a held Writable callback. Release our own
    // wait as unknown settlement, so close can join the serialized write tail.
    this.#abortWrite?.(error);
    this.#writer.destroy(error);
    this.#reader.destroy(error);
  }

  async #read(): Promise<void> {
    const frame: number[] = [];
    try {
      for await (const value of this.#reader) {
        try {
          if (!(value instanceof Uint8Array)) invalid();
          for (const byte of value) {
            frame.push(byte);
            if (frame.length > MAX_FRAME) invalid();
            if (byte === 10) {
              const line = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(new Uint8Array(frame));
              frame.length = 0;
              this.#receive(privateFrame(line));
            }
          }
        } catch {
          // Publish failure before iterator close can yield to a continuation
          // of an earlier accepted reply from this same chunk.
          this.#fail(new HostTransportError("native host response stream failed; settlement is unknown"));
          return;
        }
      }
      this.#fail(new HostTransportError("native host pipe closed; settlement is unknown"));
    } catch {
      this.#fail(new HostTransportError("native host response stream failed; settlement is unknown"));
    }
  }

  #receive(value: unknown): void {
    const response = fields(value, ["version", "id", "result"]);
    if (response.version !== 1 || typeof response.id !== "number" || !Number.isSafeInteger(response.id) || response.id < 1 || !isObject(response.result)) invalid();
    const pending = this.#pending.get(response.id);
    if (!pending) invalid();
    let result: unknown;
    let refusal: HostRefusedError | null = null;
    if (response.result.kind === "error") {
      fields(response.result, ["kind", "error"]);
      refusal = new HostRefusedError(oneOf(response.result.error, refusals));
    } else { result = pending.decode(response.result); }
    clearTimeout(pending.timer);
    this.#pending.delete(response.id);
    if (!pending.abandoned) { if (refusal) pending.reject(refusal); else pending.resolve(result); }
  }

  #startCall<T>(command: object, decode: (reply: JsonObject) => T, duration = this.#timeout, retirement = false, retainReply = false): { id: number; result: Promise<T> } {
    timeout(duration);
    if (this.#failure) throw this.#failure;
    if (this.#pending.size >= MAX_PENDING + Number(retirement) || !Number.isSafeInteger(this.#nextId)) {
      if (retirement) {
        this.#fail(new HostTransportError("Console retirement could not enter reserved capacity; settlement is unknown"));
        throw this.#failure;
      }
      throw new HostControlError("private host observation limit reached");
    }
    const requestId = this.#nextId;
    const frame = encoder.encode(serviceBody({ version: 1, id: requestId, command }) + "\n");
    if (frame.length > MAX_FRAME) throw new HostControlError("private host command exceeds limit");
    this.#nextId++;
    const result = new Promise<T>((resolve, reject) => {
      const pending: Pending = { decode, resolve: value => resolve(value as T), reject, abandoned: false, phase: "queued",
        timer: setTimeout(() => {
          // Service exchange observers have independent deadlines. The same
          // pending call retains its actual reply until response or pipe loss.
          if (retainReply) return;
          if (pending.phase !== "awaiting") { this.#fail(new HostTransportError("private host write timed out; settlement is unknown")); return; }
          pending.abandoned = true;
          reject(new HostObservationTimeout());
        }, duration),
        // This includes time queued behind an earlier write. A long human
        // observation deadline cannot keep a blocked command writer alive.
        writeTimer: setTimeout(() => {
          this.#fail(new HostTransportError("private host write timed out; settlement is unknown"));
        }, Math.min(this.#timeout, duration)) };
      this.#pending.set(requestId, pending);
      this.#writeTail = this.#writeTail.then(async () => {
        if (this.#failure) return;
        pending.phase = "writing";
        try {
          await new Promise<void>((resolve, reject) => {
            let settled = false;
            const finish = (error?: Error | null): void => {
              if (settled) return;
              settled = true; this.#abortWrite = null;
              if (error) reject(error); else resolve();
            };
            this.#abortWrite = finish;
            try { this.#writer.write(frame, finish); }
            catch (error) { finish(error instanceof Error ? error : new HostControlError("private host write failed")); }
          });
          pending.phase = "awaiting";
        }
        catch { this.#fail(new HostTransportError("private host command failed; settlement is unknown")); }
        finally { clearTimeout(pending.writeTimer); }
      });
    });
    return { id: requestId, result };
  }

  #call<T>(command: object, decode: (reply: JsonObject) => T, duration = this.#timeout): Promise<T> {
    try { return this.#startCall(command, decode, duration).result; }
    catch (error) { return Promise.reject(error); }
  }

  #ready(): void { if (this.#failure) throw this.#failure; if (this.#state !== "ready") throw new HostControlError("private host bootstrap has not completed"); }

  /** Check this client's current local channel before another transport write.
   * This is neither native authorization nor a readiness/settlement receipt.
   */
  assertReady(): void { this.#ready(); }

  /** Availability only. Pending preparation must permit the same native init;
   * ordinary host commands still require their completed private bootstrap.
   */
  assertAvailable(): void { if (this.#failure) throw this.#failure; }

  beginApplicationPreparation(preparation: ApplicationPreparation,
    options: { timeoutMs?: number } = {}): ApplicationPreparationAttempt {
    const selected = preparationData(preparation);
    const duration = timeout(options.timeoutMs ?? this.#timeout);
    if (this.#state !== "new" || this.#nextId !== 1) {
      throw new HostControlError("application preparation was already requested");
    }
    const deadline = performance.now() + duration;
    const started = this.#startCall({ kind: "prepare_application", plan: selected.plan }, reply => {
      fields(reply, ["kind", "manifest"]);
      if (reply.kind !== "application_prepared") invalid();
      return preparationArtifact(reply.manifest);
    }, duration);
    this.#state = "preparing";
    let manifest: SelectedHostArtifact | null = null;
    let selection: HostBootstrapSelection | null = null;
    let selectedBody: string | undefined;
    let adoption: Promise<void> | undefined;
    const observation = started.result.then(value => { manifest = value; return value; });
    // The native transport may not have entered init yet. A private refusal
    // must remain observed even before the caller starts awaiting this attempt.
    void observation.catch(() => {});
    return Object.freeze({
      get manifest() { return manifest; },
      get selection() { return selection; },
      observe: () => observation,
      adopt: (value: HostBootstrapSelection): Promise<void> => {
        try {
          const chosen = selectionData(value);
          const body = serviceBody(chosen);
          if (manifest === null) throw new HostControlError("application preparation has not completed");
          if (selectedBody !== undefined && selectedBody !== body) {
            throw new HostControlError("application attempt already retained another final selection");
          }
          if (adoption) return adoption;
          // Preserve the application finalizer's actual result before any
          // write. This records selection, not publication, adoption or Ready.
          selection = Object.freeze({ ...chosen,
            activation: Object.freeze(chosen.activation),
            policy_manifest: Object.freeze(chosen.policy_manifest),
            local_policy: Object.freeze(chosen.local_policy) });
          selectedBody = body;
          adoption = (async () => {
            const remaining = deadline - performance.now();
            if (remaining <= 0) throw new HostObservationTimeout();
            if (this.#state !== "preparing" || this.#nextId !== 2) {
              throw new HostControlError("application adoption is not the pending second request");
            }
            const accepted = this.#startCall({ kind: "bootstrap", selection }, reply => {
              fields(reply, ["kind"]); if (reply.kind !== "bootstrap_accepted") invalid();
            }, remaining);
            this.#state = "bootstrapping";
            await accepted.result;
            this.assertAvailable();
            this.#state = "ready";
          })();
          void adoption.catch(() => {});
          return adoption;
        } catch (error) { return Promise.reject(error); }
      },
    });
  }

  async bootstrap(selection: HostBootstrapSelection): Promise<void> {
    const selected = selectionData(selection);
    if (this.#state !== "new") throw new HostControlError("private host bootstrap was already requested");
    // Local validation must finish before changing state or publishing a request.
    // Once published, a failed bootstrap cannot leave a reusable selection.
    const started = this.#startCall({ kind: "bootstrap", selection: selected }, reply => { fields(reply, ["kind"]); if (reply.kind !== "bootstrap_accepted") invalid(); });
    this.#state = "bootstrapping";
    try {
      await started.result;
      if (this.#failure) throw this.#failure;
      this.#state = "ready";
    } catch (error) { this.#fail(new HostTransportError("private host bootstrap failed; settlement is unknown")); throw error; }
  }

  /** First command on this launcher's real private endpoints. Success selects
   * original-owner Restore; only the subsequent native init owns readiness.
   */
  async replaceApplication(startup: ApplicationUpdateStartup): Promise<void> {
    const selected = updateData(startup);
    if (this.#state !== "new") throw new HostControlError("private host bootstrap was already requested");
    const started = this.#startCall({ kind: "replace_application", startup: selected }, reply => {
      fields(reply, ["kind"]); if (reply.kind !== "bootstrap_accepted") invalid();
    });
    this.#state = "bootstrapping";
    try {
      await started.result;
      if (this.#failure) throw this.#failure;
      this.#state = "ready";
    } catch (error) { this.#fail(new HostTransportError("replacement bootstrap failed; settlement is unknown")); throw error; }
  }

  /** Connected launcher-owned composition: select on the private endpoints,
   * then send exactly one ordinary native init through the launcher's existing
   * callback-aware JSON-RPC transport. sendInit must use this same child and
   * return its actual init outcome; the launcher retains and joins all cleanup.
   * Existing runtime_options.declare_spec_update passes through unchanged.
   * This does not retrofit MetaRuntime's automatic child launch or retry init.
   */
  async startApplicationUpdate<T>(startup: ApplicationUpdateStartup, selectedBytes: Uint8Array,
    nativeParams: JsonObject, sendInit: (params: JsonObject) => Promise<T>): Promise<T> {
    const selected = updateData(startup);
    const params = snapshot(nativeParams);
    if (!(selectedBytes instanceof Uint8Array) || selectedBytes.length > 16 * 1024 * 1024
      || typeof sendInit !== "function" || ["mob_config", "application_tool_policies", "prepared_application"].some(key => Object.hasOwn(params, key))) invalid();
    const bytes = Buffer.from(selectedBytes);
    if (createHash("sha256").update(bytes).digest("hex") !== selected.selection.sha256) invalid();
    params.prepared_application = { bundle_base64: bytes.toString("base64"), digest: selected.selection.sha256 };
    await this.replaceApplication(selected);
    // No catch converts a native failure into a new attempt or successful
    // settlement. Keep this private owner alive for the launcher's native join.
    this.#ready();
    return await sendInit(params);
  }

  nextServiceParentAdministration(options: { timeoutMs?: number } = {}): Promise<ServiceParentAdministrationRequest> {
    this.#ready();
    if (this.#serviceExchange === null) {
      const exchange: ServiceParentExchange = new ServiceParentExchange(this,
        (command, decode, retirement = false) => this.#startCall(command, decode, this.#timeout, retirement, true),
        this.#timeout, () => { if (this.#serviceExchange === exchange) this.#serviceExchange = null; });
      this.#serviceExchange = exchange;
    }
    return this.#serviceExchange.observeRequest(options);
  }

  beginConsoleLogin(login: ConsoleLoginSelection, options: { timeoutMs?: number } = {}): ConsoleLoginEnrollment {
    this.#ready();
    const data = fields(snapshot(login), ["issuer", "subject", "login_scope_id", "expires_at_ms"]);
    for (const [key, bound] of [["issuer", 4096], ["subject", 4096], ["login_scope_id", 256]] as const) {
      const value = text(data[key]);
      if (!value.trim() || encoder.encode(value).length > bound || /\p{Cc}/u.test(value)) invalid();
    }
    positiveInteger(data.expires_at_ms);
    let acknowledged = false;
    let retiring = false;
    const started: { id: number; result: Promise<void> } = this.#startCall<void>({ kind: "enroll_console_login", login: data }, reply => {
      fields(reply, ["kind", "enrollment_id"]);
      if (reply.kind !== "console_login_enrolled" || positiveInteger(reply.enrollment_id) !== started.id) invalid();
      acknowledged = true;
    }, options.timeoutMs);
    void started.result.catch(() => {});
    let retirement: Promise<ConsoleLoginRetirement> | undefined;
    const enrollment: ConsoleLoginEnrollment = { enrollmentId: started.id, outcome: started.result,
      retire: (retireOptions: { timeoutMs?: number } = {}): Promise<ConsoleLoginRetirement> => {
        retiring = true;
        if (!retirement) {
          try {
            retirement = this.#startCall({ kind: "retire_console_login", enrollment_id: started.id }, reply => {
              fields(reply, ["kind", "enrollment_id", "observation"]);
              if (reply.kind !== "console_login_retired" || positiveInteger(reply.enrollment_id) !== started.id) invalid();
              const observed = fields(reply.observation, ["fenced", "reservations_drained", "uncertain"]);
              if (observed.fenced !== true || observed.reservations_drained !== true || typeof observed.uncertain !== "boolean") invalid();
              return Object.freeze({ fenced: true as const, reservations_drained: true as const, uncertain: observed.uncertain });
            }, retireOptions.timeoutMs, true).result;
          } catch (error) { retirement = Promise.reject(error); }
        }
        return retirement;
      } };
    Object.defineProperty(enrollment, consoleLoginCustody, {
      value: { client: this, enrollment, current: () => acknowledged && !retiring },
    });
    return Object.freeze(enrollment);
  }

  async retainAuthenticatedEvent(event: PrivateAuthenticatedEvent, options: { timeoutMs?: number } = {}): Promise<PrivateOriginalRetention> {
    this.#ready();
    const wire = eventData(event);
    const bytes = await globalThis.crypto.subtle.digest("SHA-256", encoder.encode(canonicalEvent(wire)));
    const expected: PrivateOriginalRef = { route: wire.route, original: wire.original,
      event_sha256: Array.from(new Uint8Array(bytes), byte => byte.toString(16).padStart(2, "0")).join("") };
    return this.#call({ kind: "retain_authenticated_event", event: wire }, reply => {
      fields(reply, ["kind", "original", "state"]);
      if (reply.kind !== "authenticated_event_retained" || reply.state !== "retained_pending_native_admission") invalid();
      const retained = original(reply.original);
      if (!sameOriginal(retained, expected)) invalid();
      return { original: retained, state: "retained_pending_native_admission" };
    }, options.timeoutMs);
  }

  /** Submit the exact retained, bound original through native admission.
   * The application authenticates the occurrence before calling. The host checks
   * SHA-256 of the exact UTF-8 content against its retained event. This command
   * cannot supply grants, model facts, requester or destination. Observation
   * timeout leaves settlement unknown and never causes an automatic resend.
   */
  async submitAuthenticatedOriginal(reference: PrivateOriginalRef, content: string,
    transportAuthentication: PrivateSourceEvidence, options: { timeoutMs?: number } = {}): Promise<PrivateOriginalSubmission> {
    this.#ready();
    const selected = original(snapshot(reference));
    const submission = { original: selected, content: text(content), transport_authentication: evidence(snapshot(transportAuthentication)) };
    return this.#call({ kind: "submit_authenticated_original", submission }, reply => {
      fields(reply, ["kind", "original", "outcome"]);
      if (reply.kind !== "original_submitted" || !sameOriginal(original(reply.original), selected)) invalid();
      if (!isObject(reply.outcome)) invalid();
      const outcome = reply.outcome;
      if (outcome.state === "admitted") {
        fields(outcome, ["state", "session_id", "input_id", "deduplicated", "turn_ticket", "source_settlement"]);
        if (typeof outcome.deduplicated !== "boolean") invalid();
        return { state: "admitted", session_id: identifier(outcome.session_id), input_id: identifier(outcome.input_id),
          deduplicated: outcome.deduplicated, turn_ticket: outcome.turn_ticket === null ? null : uuid(outcome.turn_ticket),
          source_settlement: oneOf(outcome.source_settlement, ["fresh_acceptance_promoted", "retry_withdrawn", "post_admission_publication_failed"]) };
      }
      fields(outcome, ["state"]);
      return { state: oneOf(outcome.state, ["admission_unresolved", "native_outcome_unavailable"]) };
    }, options.timeoutMs);
  }

  /** Read one canonical approved B without submitting or waking work. Pending,
   * absent receipt and unavailable read differ. Preserve witness/batch/scope and
   * native outcome; Run scope alone does not establish Completed(result).
   * Oversized results are bounded unavailable replies, never truncated.
   */
  async observeServiceInputReceipt(reference: PrivateOriginalRef,
    options: { timeoutMs?: number } = {}): Promise<ServiceInputObservation> {
    this.#ready();
    const selected = original(snapshot(reference));
    return this.#call({ kind: "observe_service_input_receipt", original: selected }, reply => {
      fields(reply, ["kind", "original", "observation"]);
      if (reply.kind !== "service_input_receipt" || !sameOriginal(original(reply.original), selected)) invalid();
      return serviceInputObservation(reply.observation);
    }, options.timeoutMs);
  }

  async cancelOriginalOperation(reference: PrivateOriginalRef, options: { timeoutMs?: number } = {}): Promise<PrivateOriginalCancellation> {
    this.#ready();
    const selected = original(snapshot(reference));
    return this.#call({ kind: "cancel_original_operation", original: selected }, reply => {
      fields(reply, ["kind", "original", "state"]);
      if (reply.kind !== "original_operation_cancelled" || reply.state !== "operations_cancelled") invalid();
      const retained = original(reply.original);
      if (!sameOriginal(retained, selected)) invalid();
      return { original: retained, state: "operations_cancelled" };
    }, options.timeoutMs);
  }

  /** Begin once, then observe/answer on this same connection while outcome is pending.
   * Authenticate and claim the exact human answer in the application before forwarding it.
   * A timeout never retries review, approval, issuance or the original task.
   */
  beginRemoteControllerReview(reference: PrivateOriginalRef, content: string,
    transportAuthentication: PrivateSourceEvidence, login: RemoteControllerLogin,
    options: { timeoutMs?: number } = {}): RemoteControllerReview {
    this.#ready();
    const selected = original(snapshot(reference));
    const invocation = fields(snapshot(login), ["scope_id", "expires_at_ms"]);
    const wireLogin = { scope_id: identifier(invocation.scope_id), expires_at_ms: positiveInteger(invocation.expires_at_ms) };
    const submission = { original: selected, content: text(content), transport_authentication: evidence(snapshot(transportAuthentication)) };
    const started = this.#startCall({ kind: "review_remote_controller_parent", submission, login: wireLogin }, reply => {
      fields(reply, ["kind", "outcome"]); if (reply.kind !== "controller_parent_reviewed") invalid();
      return controllerOutcome(reply.outcome, selected.route);
    }, options.timeoutMs);
    // Consumers normally observe the prompt before awaiting outcome. Retain its
    // rejection without an unhandled-rejection event in that intentional interval.
    void started.result.catch(() => {});
    return Object.freeze({ reviewId: started.id, outcome: started.result });
  }

  /** A distinct review, on the same retained transport, before this original's first bind. */
  beginRemoteSetupRequestReview(reference: PrivateOriginalRef, content: string,
    transportAuthentication: PrivateSourceEvidence, login: RemoteControllerLogin,
    options: { timeoutMs?: number } = {}): RemoteSetupRequestReview {
    this.#ready();
    const selected = original(snapshot(reference));
    const invocation = fields(snapshot(login), ["scope_id", "expires_at_ms"]);
    const wireLogin = { scope_id: identifier(invocation.scope_id), expires_at_ms: positiveInteger(invocation.expires_at_ms) };
    const submission = { original: selected, content: text(content), transport_authentication: evidence(snapshot(transportAuthentication)) };
    const started = this.#startCall<SetupRequestOutcome>({ kind: "review_remote_setup_request", submission, login: wireLogin }, reply => {
      fields(reply, ["kind", "outcome"]); if (reply.kind !== "setup_request_reviewed") invalid();
      const value = reply.outcome;
      if (!isObject(value)) invalid();
      if (value.state !== "setup_request_selected") {
        fields(value, ["state"]);
        return { state: oneOf(value.state, ["denied", "unavailable", "pending_reconciliation"]) };
      }
      fields(value, ["state", "selection"]);
      if (!isObject(value.selection)) invalid();
      const hasTargets = Object.hasOwn(value.selection, "helper_targets");
      const chosen = fields(value.selection, ["original", "session_id", "parent", ...(hasTargets ? ["helper_targets"] : [])]);
      const retained = original(chosen.original);
      if (!sameOriginal(retained, selected)) invalid();
      const parent = nativeLineage(chosen.parent); if (parent.length !== 1) invalid();
      return { state: "setup_request_selected", selection: {
        original: retained, session_id: uuid(chosen.session_id), parent,
        ...(hasTargets ? { helper_targets: setupHelperTargets(chosen.helper_targets) } : {}),
      } };
    }, options.timeoutMs);
    void started.result.catch(() => {});
    return Object.freeze({ reviewId: started.id, outcome: started.result });
  }

  #controllerReviewCall(kind: string, reviewId: number, allowed: readonly string[], extra: object = {}): Promise<ControllerReviewStatus> {
    this.#ready(); const selected = positiveInteger(reviewId);
    return this.#call({ kind, review_id: selected, ...extra }, reply => {
      fields(reply, ["kind", "review_id", "status"]);
      if (reply.kind !== "controller_review" || positiveInteger(reply.review_id) !== selected) invalid();
      return controllerStatus(reply.status, allowed);
    });
  }
  observeControllerReview(reviewId: number): Promise<ControllerReviewStatus> {
    return this.#controllerReviewCall("observe_controller_review", reviewId, ["awaiting_prompt", "prompt", "answer_forwarded"]);
  }
  answerControllerReview(reviewId: number, approvalId: string, decision: "approve" | "deny"): Promise<ControllerReviewStatus> {
    return this.#controllerReviewCall("answer_controller_review", reviewId, ["answer_forwarded"],
      { approval_id: identifier(approvalId), decision: oneOf(decision, ["approve", "deny"]) });
  }
  retireControllerReview(reviewId: number): Promise<ControllerReviewStatus> {
    return this.#controllerReviewCall("retire_controller_review", reviewId, ["retired"]);
  }

  async openComponent(component: string): Promise<HostComponentObservation> {
    this.#ready(); const selected = id(component);
    return this.#call({ kind: "open_component", component: selected }, reply => {
      fields(reply, ["kind", "observation"]); if (reply.kind !== "component") invalid();
      const result = fields(reply.observation, ["incarnation", "component", "revision", "state"]);
      if (id(result.component) !== selected || typeof result.revision !== "number" || !Number.isSafeInteger(result.revision) || result.revision < 0) invalid();
      return { incarnation: uuid(result.incarnation), component: selected, revision: result.revision,
        state: oneOf(result.state, ["staged", "open", "closing", "closed", "quarantined", "uncertain", "failed"]) };
    });
  }

  #closeReply(command: object, selectedScope: HostCloseScope, selectedHandle?: HostCloseHandle, duration?: number): Promise<HostCloseObservation> {
    this.#ready();
    return this.#call(command, reply => {
      fields(reply, ["kind", "observation"]); if (reply.kind !== "close") invalid();
      const result = closeObservation(reply.observation);
      if (!sameScope(result.handle.scope, selectedScope) || (selectedHandle && (result.handle.incarnation !== selectedHandle.incarnation || result.handle.close_id !== selectedHandle.close_id))) invalid();
      return result;
    }, duration);
  }
  beginCloseComponent(component: string, reason: HostCloseReason): Promise<HostCloseObservation> {
    const selected = id(component);
    return this.#closeReply({ kind: "begin_close_component", component: selected, reason: this.#reason(reason) }, { kind: "component", component: selected });
  }
  beginCloseActivation(reason: HostCloseReason): Promise<HostCloseObservation> {
    return this.#closeReply({ kind: "begin_close_activation", reason: this.#reason(reason) }, { kind: "activation" });
  }
  observeClose(selected: HostCloseHandle): Promise<HostCloseObservation> {
    const retained = handle(snapshot(selected));
    return this.#closeReply({ kind: "observe_close", handle: retained }, retained.scope, retained);
  }
  joinClose(selected: HostCloseHandle, options: { timeoutMs?: number } = {}): Promise<HostCloseObservation> {
    const retained = handle(snapshot(selected));
    return this.#closeReply({ kind: "join_close", handle: retained }, retained.scope, retained, options.timeoutMs);
  }
  #reason(value: HostCloseReason): HostCloseReason { return oneOf(value, ["shutdown", "operator_disable", "quarantine", "activation_replaced", "host_lost"]); }

  async close(): Promise<void> {
    this.#fail(new HostTransportError("local host stream closed; settlement is unknown"));
    await this.#readerTask;
    await this.#writeTail;
  }
}

// Shared local startup validation for the ordinary SDK transport composition.
// Copies contain wire selections only, never authenticated authority.
export { selectionData as copyHostBootstrapSelection, updateData as copyApplicationUpdateStartup,
  preparationData as copyApplicationPreparation };
