/**
 * Long-term agents from host code (`meta/identity/*`).
 *
 * A long-term agent is a durable, named member of the mob: it keeps its own
 * conversation and memory across restarts and is addressed by name. An agent
 * made at run time is the same kind of agent as one the host's configuration
 * declares, and every agent is read and changed through the same calls:
 * `MetaRuntime.identities` (an `IdentitiesHandle`) has one call per host
 * method.
 *
 * Writes: `create` (1 to 256 agents, all or nothing), `update`, `archive`,
 * `unarchive`, `remove`, `adopt`, `settlePreparation`, `withdraw` and
 * `changesHold`. Reads: `list`, `get`, `operation`, `directory` and `plan`. A
 * read never wakes an agent.
 *
 * Requests are spelled once, here, as every surface writes them: a create
 * names `agents: [{name, profile, ...}]` ({@link AgentSpec}), a change of
 * one agent names it `name`, and a refusal's `field` is a path into that
 * request under the wire's names (`agents[1].name`, `set.labels`,
 * `peers_add[0]`).
 *
 * A change resolves one typed outcome, discriminated by `outcome`:
 *
 * - `done`: the change committed;
 * - `pending_decision`: a person must decide. This is a normal answer, not
 *   an error: nothing but the decision is written, and the same `requestId`
 *   and request return the same state. Read the operation later with
 *   `identities.operation({ operationId: outcome.operationId })`;
 * - `pending_preparation`: committed, not usable until the host has prepared
 *   the agent and settled it (`settlePreparation`);
 * - a refusal rejects with `IdentityRefusedError`, the one error of the
 *   family, with the typed `kind`, `field` and `detail`. Only `operation`
 *   resolves a refusal as a value (`refused`): the operation it read ended
 *   with one.
 *
 * Vocabulary: refusal kinds (`IDENTITY_REFUSAL_KINDS`), the outcome tokens
 * ({@link IDENTITY_OUTCOMES}), an instruction outcome's kinds and reasons
 * ({@link INSTRUCTION_OUTCOME_KINDS}, {@link INSTRUCTION_NEXT_SESSION_REASONS})
 * and a seat ({@link AGENT_SEATS}) are closed: a new value is a breaking
 * change, and each list is checked against the gateway's own. A gateway
 * newer than this SDK may still send one this SDK does not list: it reaches
 * you as its token, never as a throw. Advisory values (an agent's
 * `readiness`, a change's `state`, a provenance's `entered` and consent, a
 * directory verdict, a plan's `outcome` and `change`, an agent's
 * `addressability` and `runtimeModeOverride` as read back, the `effect` on an
 * agent's schedules) are open: a value this SDK does not know is kept as the
 * gateway sent it.
 */
// errors.ts imports nothing, so this runtime import adds no cycle.
import { ContractMismatchError, parseIdentityRefusal, type IdentityRefusal } from "./errors.js";

/** The outcome tokens of a change, a closed vocabulary. */
export const IDENTITY_OUTCOMES = [
  "done",
  "pending_decision",
  "pending_preparation",
  "refused",
] as const;
/** What became of an update's instructions change, a closed vocabulary. */
export const INSTRUCTION_OUTCOME_KINDS = [
  "applies_at_next_run",
  "applies_after_compaction",
  "applies_at_next_session",
  "unchanged",
] as const;
export type InstructionOutcomeKind = (typeof INSTRUCTION_OUTCOME_KINDS)[number];
/**
 * Why an instructions change applies only when the agent next starts afresh,
 * a closed vocabulary.
 */
export const INSTRUCTION_NEXT_SESSION_REASONS = [
  "prepared_authorization_required",
  "slot_limit_exceeded",
  "active_set_budget_exceeded",
  "lowering_unavailable",
  "body_too_large",
  "record_unavailable",
] as const;
export type InstructionNextSessionReason = (typeof INSTRUCTION_NEXT_SESSION_REASONS)[number];
/** Whether the agent a create committed is seated, a closed vocabulary. */
export const AGENT_SEATS = ["seated", "owed"] as const;
export type AgentSeat = (typeof AGENT_SEATS)[number];
/** Who reaches an agent, as a create and an update set it. */
export type AgentAddressability = "addressable" | "internal_only";
/** How an agent runs, as a create and an update set it. */
export type AgentRuntimeMode = "autonomous_host" | "turn_driven";
/** The changes of the family, as `plan` names them. */
export const IDENTITY_CHANGES = [
  "create",
  "update",
  "archive",
  "unarchive",
  "remove",
  "adopt",
] as const;
export type IdentityChange = (typeof IDENTITY_CHANGES)[number];

// -- Requests ---------------------------------------------------------------

/**
 * Host-attested evidence of where a request came from. Recorded with the
 * change, never authority.
 */
export interface SourceEvidence {
  attestor: string;
  subject: string;
  sourceRef: string;
  observedAtMs: number;
}

/**
 * What host code adds to every change. `requestId` names the change: the
 * same id and request return the same state and commit at most once, so a
 * retry after a lost answer is safe, and another request under the same id
 * is refused `operation_reused`. `proposer` and `sourceEvidence` are
 * recorded with the change, never authority.
 */
export interface IdentityChangeOptions {
  requestId: string;
  proposer?: string;
  sourceEvidence?: SourceEvidence;
}

/**
 * One agent of a create.
 *
 * `name` is unique in the mob and never available again once the agent is
 * removed. `profile` is one of the mob's profiles: it gives the agent's
 * model, tools and skills. Optional: `displayName`; `instructions`, its
 * standing brief; `context`, descriptive and never authority; `labels`;
 * `firstMessage`, its first turn, run once (text, or a list of content
 * blocks); `peers`, the agents it can message, by name (agents of the mob,
 * or other agents of the same create); `addressability`, who reaches it
 * (`addressable`, which leaving it out means, or `internal_only`);
 * `runtimeModeOverride`, how it runs (`autonomous_host` or `turn_driven`;
 * left out, as its profile says). The last two are the members and values
 * of a roster row, with the same meaning.
 */
export interface AgentSpec {
  name: string;
  profile: string;
  displayName?: string;
  instructions?: readonly string[];
  context?: unknown;
  labels?: Readonly<Record<string, string>>;
  firstMessage?: string | readonly Readonly<Record<string, unknown>>[];
  peers?: readonly string[];
  addressability?: AgentAddressability;
  runtimeModeOverride?: AgentRuntimeMode;
}

/** A create of 1 to 256 agents, all or nothing. They may name each other as peers. */
export interface CreateAgents extends IdentityChangeOptions {
  agents: readonly AgentSpec[];
}

/**
 * What an update sets. A member left out is unchanged. `instructions` and
 * `context` are replaced whole. `labels` sets each key it names and keeps
 * the agent's other labels; `labelsRemove` names keys to remove. A reserved
 * label the agent carries (one of Meta's own, or one the policy reserves,
 * that its configuration gave it before its adoption) stays through an
 * update that does not name it; an update that would change or remove it is
 * refused `reserved_key` at `set.labels`, the removal of one of Meta's own
 * labels at `set.labels_remove`. A label
 * change reaches the agent's member at once while the member is idle, on the
 * same conversation; a member that held work then takes it at a retry of
 * the update, at its next update or at its next seat. `profile`
 * moves the agent to that profile's model and tools, on the same
 * conversation. `addressability` and `runtimeModeOverride` take the values
 * they take on a create ({@link AgentSpec}); an update sets a runtime mode
 * and cannot return the agent to its profile's.
 */
export interface AgentSet {
  displayName?: string;
  instructions?: readonly string[];
  context?: unknown;
  labels?: Readonly<Record<string, string>>;
  labelsRemove?: readonly string[];
  profile?: string;
  addressability?: AgentAddressability;
  runtimeModeOverride?: AgentRuntimeMode;
}

/**
 * A change of one agent as a whole: its archive, its unarchive, its removal
 * or its adoption. `expectedSpecDigest` is the agent's `specDigest` as you
 * read it (`get` or `list`): an agent that changed meanwhile refuses
 * `stale_spec` with the current one. There is one spec digest on every
 * surface, the digest of the agent's record: the same value in `list`,
 * `get`, a change's outcome and `stale_spec`'s `current_digest`. An adoption
 * does not change it.
 */
export interface AgentChangeOptions extends IdentityChangeOptions {
  expectedSpecDigest: string;
}

/** An update of one agent: what it sets, and the peers it adds and removes. */
export interface UpdateAgentOptions extends AgentChangeOptions {
  set?: AgentSet;
  peersAdd?: readonly string[];
  peersRemove?: readonly string[];
}

/** A change of one agent as a whole, as `plan` takes it. */
export interface AgentRef extends AgentChangeOptions {
  name: string;
}

/** An update of one agent, as `plan` takes it. */
export interface UpdateAgent extends UpdateAgentOptions {
  name: string;
}

/**
 * An operation, named by your own request's `requestId` or by the
 * `operationId` an outcome named: exactly one.
 */
export type OperationRef =
  | { requestId: string; operationId?: undefined }
  | { operationId: string; requestId?: undefined };

/** Which agents a `list` reads. */
export interface ListAgentsOptions {
  profile?: string;
  includeArchived?: boolean;
  after?: string;
}

/** Which entries a `directory` reads. */
export interface DirectoryOptions {
  query?: string;
  after?: string;
}

function requestId(value: unknown): string {
  if (typeof value !== "string" || value.trim() === "") {
    throw new TypeError(
      "requestId is required: host code names each change, so that the same id " +
        "and request return the same state",
    );
  }
  return value;
}

function envelope(
  params: Record<string, unknown>,
  options: IdentityChangeOptions,
): Record<string, unknown> {
  if (options.proposer !== undefined) params.proposer = options.proposer;
  if (options.sourceEvidence !== undefined) {
    params.source_evidence = {
      attestor: options.sourceEvidence.attestor,
      subject: options.sourceEvidence.subject,
      source_ref: options.sourceEvidence.sourceRef,
      observed_at_ms: options.sourceEvidence.observedAtMs,
    };
  }
  return params;
}

function agentSpecParams(agent: AgentSpec): Record<string, unknown> {
  const params: Record<string, unknown> = { name: agent.name, profile: agent.profile };
  if (agent.displayName !== undefined) params.display_name = agent.displayName;
  if (agent.instructions !== undefined && agent.instructions.length > 0) {
    params.instructions = [...agent.instructions];
  }
  if (agent.context !== undefined && agent.context !== null) params.context = agent.context;
  if (agent.labels !== undefined && Object.keys(agent.labels).length > 0) {
    params.labels = { ...agent.labels };
  }
  if (agent.firstMessage !== undefined) {
    params.first_message =
      typeof agent.firstMessage === "string"
        ? agent.firstMessage
        : agent.firstMessage.map((block) => ({ ...block }));
  }
  if (agent.peers !== undefined && agent.peers.length > 0) params.peers = [...agent.peers];
  if (agent.addressability !== undefined) params.addressability = agent.addressability;
  if (agent.runtimeModeOverride !== undefined) {
    params.runtime_mode_override = agent.runtimeModeOverride;
  }
  return params;
}

/** @internal The params of `meta/identity/create`. */
export function createAgentsParams(request: CreateAgents): Record<string, unknown> {
  return envelope(
    {
      request_id: requestId(request.requestId),
      agents: request.agents.map(agentSpecParams),
    },
    request,
  );
}

function agentSetParams(set: AgentSet): Record<string, unknown> {
  const params: Record<string, unknown> = {};
  if (set.displayName !== undefined) params.display_name = set.displayName;
  if (set.instructions !== undefined) params.instructions = [...set.instructions];
  if (set.context !== undefined && set.context !== null) params.context = set.context;
  if (set.labels !== undefined) params.labels = { ...set.labels };
  if (set.labelsRemove !== undefined && set.labelsRemove.length > 0) {
    params.labels_remove = [...set.labelsRemove];
  }
  if (set.profile !== undefined) params.profile = set.profile;
  if (set.addressability !== undefined) params.addressability = set.addressability;
  if (set.runtimeModeOverride !== undefined) {
    params.runtime_mode_override = set.runtimeModeOverride;
  }
  return params;
}

/** @internal The params of `meta/identity/update`. */
export function updateAgentParams(request: UpdateAgent): Record<string, unknown> {
  const params: Record<string, unknown> = {
    request_id: requestId(request.requestId),
    name: request.name,
    expected_spec_digest: request.expectedSpecDigest,
    set: agentSetParams(request.set ?? {}),
  };
  if (request.peersAdd !== undefined && request.peersAdd.length > 0) {
    params.peers_add = [...request.peersAdd];
  }
  if (request.peersRemove !== undefined && request.peersRemove.length > 0) {
    params.peers_remove = [...request.peersRemove];
  }
  return envelope(params, request);
}

/** @internal The params of an archive, an unarchive, a removal or an adoption. */
export function agentRefParams(request: AgentRef): Record<string, unknown> {
  return envelope(
    {
      request_id: requestId(request.requestId),
      name: request.name,
      expected_spec_digest: request.expectedSpecDigest,
    },
    request,
  );
}

/**
 * @internal The params of `meta/identity/settle_preparation`: the agent and
 * the caller's id of the operation, and nothing else (the gateway accepts no
 * other member).
 */
export function settlePreparationParams(
  name: string,
  options: { requestId: string },
): Record<string, unknown> {
  return { name, request_id: requestId(options.requestId) };
}

/** @internal The params that name one operation. */
export function operationRefParams(operation: OperationRef): Record<string, unknown> {
  const named = operation as { requestId?: unknown; operationId?: unknown };
  if ((named.requestId === undefined) === (named.operationId === undefined)) {
    throw new TypeError("name exactly one of requestId and operationId");
  }
  return named.requestId !== undefined
    ? { request_id: named.requestId }
    : { operation_id: named.operationId };
}

/** @internal The params of `meta/identity/plan`. */
export function planParams(
  change: IdentityChange,
  request: CreateAgents | UpdateAgent | AgentRef,
): Record<string, unknown> {
  if (!(IDENTITY_CHANGES as readonly string[]).includes(change)) {
    throw new TypeError(`change is one of ${IDENTITY_CHANGES.join(", ")}`);
  }
  const params =
    change === "create"
      ? createAgentsParams(request as CreateAgents)
      : change === "update"
        ? updateAgentParams(request as UpdateAgent)
        : agentRefParams(request as AgentRef);
  params.change = change;
  return params;
}

// -- Answers ----------------------------------------------------------------

function object(raw: unknown, what: string): Record<string, unknown> {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw new ContractMismatchError(`${what} must be an object`);
  }
  return raw as Record<string, unknown>;
}

function text(data: Record<string, unknown>, key: string, what: string): string {
  const value = data[key];
  if (typeof value !== "string") throw new ContractMismatchError(`${what} has no ${key}`);
  return value;
}

function integer(data: Record<string, unknown>, key: string, what: string): number {
  const value = data[key];
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new ContractMismatchError(`${what} has no ${key}`);
  }
  return value;
}

function optionalText(data: Record<string, unknown>, key: string): string | null {
  const value = data[key];
  return typeof value === "string" ? value : null;
}

function optionalInteger(data: Record<string, unknown>, key: string): number | null {
  const value = data[key];
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function strings(raw: unknown): string[] {
  return Array.isArray(raw) ? raw.filter((item): item is string => typeof item === "string") : [];
}

/** One schedule, as an answer names it: its id, and its name when it has one. */
export interface ScheduleRef {
  readonly scheduleId: string;
  readonly name: string | null;
}

/**
 * An agent's schedules as a change of the agent leaves them or, in a plan,
 * would leave them.
 *
 * The schedules that target an agent follow three changes, so that no
 * schedule keeps firing at an agent that takes no delivery: an archive
 * pauses its Active schedules, an unarchive resumes exactly the schedules
 * that were paused that way (one somebody paused by name stays paused), and
 * a removal ends every one. `effect` says which (advisory; today `paused`,
 * `resumed`, `ended`), `count` how many and `schedules` which, as they stand
 * after the change: after an archive the schedules that are paused for the
 * agent, after an unarchive the agent's schedules that are active (one
 * somebody resumed by name while the agent was archived included), after a
 * removal the ended ones. A schedule somebody paused by name is on neither
 * of the first two lists. `incomplete` is `true` when a schedule could not
 * be read or changed: the same request asked again follows the rest.
 *
 * `effect` and `count` are told to every caller of the change. The list is
 * the host's: the stdio gateway, which this SDK calls, gives it, and the
 * Console gives it to a caller that administers the runtime
 * (`runtime.admin`). An agent that asks for the change through its tools,
 * and any other Console caller, is answered without the list, since a
 * schedule that targets an agent may be anybody's. An answer without the
 * list reads here as an empty `schedules`, and `count` still says how many:
 * do not take the list's length for the count.
 */
export interface SchedulesFollowed {
  readonly effect: string;
  readonly count: number;
  readonly schedules: readonly ScheduleRef[];
  readonly incomplete: boolean;
}

function parseSchedulesFollowed(data: Record<string, unknown>): SchedulesFollowed | null {
  if (data.schedules === undefined || data.schedules === null) return null;
  const what = "an agent's schedules";
  const followed = object(data.schedules, what);
  const schedules = Array.isArray(followed.schedules) ? followed.schedules : [];
  return {
    effect: text(followed, "effect", what),
    count: integer(followed, "count", what),
    schedules: schedules.map((entry) => {
      const schedule = object(entry, "a schedule");
      return {
        scheduleId: text(schedule, "schedule_id", "a schedule"),
        name: optionalText(schedule, "name"),
      };
    }),
    incomplete: followed.incomplete === true,
  };
}

/**
 * What became of an update's instructions change.
 *
 * The new text is in the agent's record at once, whatever the outcome. It
 * reaches the agent at its next run; a run that starts within moments of the
 * change may still use the earlier instructions; earlier instructions are
 * never rewritten.
 *
 * `kind` is one of {@link INSTRUCTION_OUTCOME_KINDS}: `applies_at_next_run`;
 * `applies_after_compaction` (the agent's model takes new instructions only
 * at the head of what it reads); `applies_at_next_session` (the change does
 * not reach the agent as it runs now and applies when it next starts afresh,
 * for `reason`, one of {@link INSTRUCTION_NEXT_SESSION_REASONS}); `unchanged`.
 */
export interface InstructionOutcome {
  readonly kind: InstructionOutcomeKind | (string & {});
  readonly reason: InstructionNextSessionReason | (string & {}) | null;
}

/**
 * One agent of a committed change.
 *
 * `state` says what the change did to it (advisory; today `committed`,
 * `pending_preparation`, `archiving`, `archived`, `restored`, `retiring`,
 * `removed`). `specDigest` is its spec digest after the change, what its
 * next change names as `expectedSpecDigest`; `null` for a removed agent.
 * `seat` is on a create's outcome only, one of {@link AGENT_SEATS}: `owed`
 * means the agent is committed and durable and its first message did not
 * start; it is seated without a restart at its next delivery.
 * `instructions` is on an update's outcome only. `schedules` says how the
 * agent's schedules stand after the change ({@link SchedulesFollowed}):
 * it is on the first answer of an archive, an unarchive and a removal, on a
 * launch whose host gives the family its schedules (the stdio gateway does).
 *
 * `seat` and `sessionId` say how the agent stands when the outcome is
 * answered: a replay of the request, and a later `operation` read, answer
 * today's session, and no `seat` once the agent is neither seated nor owed
 * its seat (it was archived or removed since). A replay and an `operation`
 * read state no `schedules` either, so do not compare a retry's answer whole
 * with the first one: the `operationId` and each agent's `state` and
 * `specDigest` are what stays.
 */
export interface AgentOutcome {
  readonly name: string;
  readonly state: string;
  readonly specDigest: string | null;
  readonly specRevision: number | null;
  readonly sessionId: string | null;
  readonly seat: AgentSeat | (string & {}) | null;
  readonly instructions: InstructionOutcome | null;
  readonly schedules: SchedulesFollowed | null;
}

function parseAgentOutcome(raw: unknown): AgentOutcome {
  const what = "an agent outcome";
  const data = object(raw, what);
  let instructions: InstructionOutcome | null = null;
  if (data.instructions !== undefined && data.instructions !== null) {
    const outcome = object(data.instructions, "an instruction outcome");
    instructions = {
      kind: text(outcome, "kind", "an instruction outcome"),
      reason: optionalText(outcome, "reason"),
    };
  }
  return {
    name: text(data, "name", what),
    state: text(data, "state", what),
    specDigest: optionalText(data, "spec_digest"),
    specRevision: optionalInteger(data, "spec_revision"),
    sessionId: optionalText(data, "session_id"),
    seat: optionalText(data, "seat"),
    instructions,
    schedules: parseSchedulesFollowed(data),
  };
}

function agentOutcomes(data: Record<string, unknown>): AgentOutcome[] {
  if (!Array.isArray(data.agents)) {
    throw new ContractMismatchError("a committed outcome has no agents");
  }
  return data.agents.map(parseAgentOutcome);
}

/** The change committed. `agents` are the agents it changed, in the order the request names them. */
export interface IdentityDone {
  readonly outcome: "done";
  readonly operationId: string;
  readonly agents: readonly AgentOutcome[];
}

/**
 * A decision is held for the change: a person must decide it. Nothing else
 * is written. The same `requestId` and request return this state again, so
 * do not repeat the change under a new id. Read what became of it with
 * `identities.operation({ operationId })`; `deadlineMs` is when the decision
 * expires (UNIX ms).
 */
export interface IdentityPendingDecision {
  readonly outcome: "pending_decision";
  readonly operationId: string;
  readonly decisionId: string;
  readonly deadlineMs: number;
}

/**
 * The change committed, and at least one of its agents is not usable yet:
 * the host prepares what the agent needs first.
 *
 * A create answers it for an agent of a profile whose preparation the
 * launch's policy names `required`. Such an agent reads `readiness`
 * `preparation_pending`, takes no delivery (a retryable refusal), runs
 * nothing and wakes nothing. Prepare what it needs, then call
 * `identities.settlePreparation(name, { requestId })`: the agent is seated
 * dormant and its first message, if it has one, runs once. A host that
 * cannot prepare an agent leaves it pending, or removes it.
 */
export interface IdentityPendingPreparation {
  readonly outcome: "pending_preparation";
  readonly operationId: string;
  readonly agents: readonly AgentOutcome[];
}

/**
 * The change was refused: nothing of it was written. A change call rejects
 * with this as `IdentityRefusedError`; only `operation` resolves it as a
 * value.
 */
export interface IdentityRefused {
  readonly outcome: "refused";
  readonly refusal: IdentityRefusal;
}

/**
 * No change committed under the operation and no decision is held for it: it
 * was never asked, or it was refused. Asking it commits it at most once.
 * `operationId` is the operation the read named.
 */
export interface IdentityOperationUnknown {
  readonly outcome: "unknown";
  readonly operationId: string;
}

/** The typed outcome of a change. */
export type IdentityOutcome =
  | IdentityDone
  | IdentityPendingDecision
  | IdentityPendingPreparation
  | IdentityRefused;
/** What a change call resolves: every outcome but a refusal, which it rejects with. */
export type IdentityChangeResult =
  | IdentityDone
  | IdentityPendingDecision
  | IdentityPendingPreparation;
/** What `operation` resolves. */
export type IdentityOperation = IdentityOutcome | IdentityOperationUnknown;

/**
 * The typed outcome of a change, from the gateway's answer. Throws
 * `ContractMismatchError` for an answer that is no outcome this SDK reads: an
 * outcome token outside {@link IDENTITY_OUTCOMES}, or an outcome without its
 * members.
 */
export function parseIdentityOutcome(raw: unknown): IdentityOutcome {
  const data = object(raw, "an identity outcome");
  const token = data.outcome;
  if (token === "done") {
    return {
      outcome: "done",
      operationId: text(data, "operation_id", "a done outcome"),
      agents: agentOutcomes(data),
    };
  }
  if (token === "pending_decision") {
    const what = "a pending_decision outcome";
    return {
      outcome: "pending_decision",
      operationId: text(data, "operation_id", what),
      decisionId: text(data, "decision_id", what),
      deadlineMs: integer(data, "deadline_ms", what),
    };
  }
  if (token === "pending_preparation") {
    return {
      outcome: "pending_preparation",
      operationId: text(data, "operation_id", "a pending_preparation outcome"),
      agents: agentOutcomes(data),
    };
  }
  if (token === "refused") {
    const refusal = parseIdentityRefusal(data);
    if (refusal === null) throw new ContractMismatchError("a refused outcome has no kind");
    return { outcome: "refused", refusal };
  }
  throw new ContractMismatchError(
    `identity outcome ${JSON.stringify(token)} is none this SDK reads ` +
      `(${IDENTITY_OUTCOMES.join(", ")})`,
  );
}

/** @internal What `meta/identity/operation` answers. */
export function parseIdentityOperation(raw: unknown): IdentityOperation {
  if (
    typeof raw === "object" &&
    raw !== null &&
    (raw as Record<string, unknown>).outcome === "unknown"
  ) {
    return {
      outcome: "unknown",
      operationId: text(raw as Record<string, unknown>, "operation_id", "an unknown operation"),
    };
  }
  return parseIdentityOutcome(raw);
}

/**
 * How, when and at whose request an agent came to exist. Recorded and shown;
 * nothing reads it for a decision.
 *
 * `entered` is advisory (today `created`, `adopted`, `imported`).
 * `requesterKind` is the kind of requester that asked (`agent`, `host`,
 * `human`) and `requester` who: an agent's name, a host surface, or a
 * person's principal key. `consent` is the consent the change committed
 * under, as the gateway sent it; `consentForm` is its form (advisory; today
 * `allowed`, `notified`, `decided`, `imported`).
 */
export interface AgentProvenance {
  readonly entered: string;
  readonly requesterKind: string;
  readonly requester: string;
  readonly atMs: number;
  readonly consent: Readonly<Record<string, unknown>>;
  readonly consentForm: string | null;
}

function parseAgentProvenance(raw: unknown): AgentProvenance {
  const what = "an agent's provenance";
  const data = object(raw, what);
  const consent =
    typeof data.consent === "object" && data.consent !== null && !Array.isArray(data.consent)
      ? { ...(data.consent as Record<string, unknown>) }
      : {};
  return {
    entered: text(data, "entered", what),
    requesterKind: text(data, "requester_kind", what),
    requester: text(data, "requester", what),
    atMs: integer(data, "at_ms", what),
    consent,
    consentForm: typeof consent.form === "string" ? consent.form : null,
  };
}

/**
 * One agent as a list names it. Every agent of the mob is listed the same
 * way, whoever asked for it and by whatever route it came to exist.
 *
 * `specDigest` is what a change names as `expectedSpecDigest`. `readiness`
 * says where its member stands (advisory; today `dormant`, `live`, `broken`,
 * `preparation_pending`, `retiring`, `unseated`). `openOperations` are the
 * ids of the operations open for it: its create while it waits for its
 * host's preparation (until the settle), and each change of it that is held
 * for a person's decision or approved and not yet carried out; empty when
 * nothing is open. Read one with `identities.operation({ operationId })`.
 * `addressability` and `runtimeModeOverride` read back what a create or an
 * update set, each only when it is not the default: `addressability` is
 * `internal_only` or `null` (the agent is addressable), and
 * `runtimeModeOverride` is `null` for an agent that runs as its profile
 * says. `rosterListingSuperseded` is `true` when the host's roster lists
 * this name while the agent is served from its record (an adopted agent
 * whose roster still lists it, for one): the roster's entry for the name is
 * not applied, and a change of the agent is a change of its record. The
 * gateway writes it only when it is so; an answer this SDK parsed always
 * carries it, `false` when the gateway left it out.
 */
export interface AgentSummary {
  readonly name: string;
  readonly profile: string;
  readonly specDigest: string;
  readonly archived: boolean;
  readonly readiness: string;
  readonly displayName: string | null;
  readonly specRevision: number | null;
  readonly provenance: AgentProvenance | null;
  readonly openOperations: readonly string[];
  readonly addressability: AgentAddressability | (string & {}) | null;
  readonly runtimeModeOverride: AgentRuntimeMode | (string & {}) | null;
  readonly rosterListingSuperseded?: boolean;
}

function parseAgentSummaryOf(data: Record<string, unknown>, what: string): AgentSummary {
  if (typeof data.archived !== "boolean") {
    throw new ContractMismatchError(`${what} has no archived`);
  }
  return {
    name: text(data, "name", what),
    profile: text(data, "profile", what),
    specDigest: text(data, "spec_digest", what),
    archived: data.archived,
    readiness: text(data, "readiness", what),
    displayName: optionalText(data, "display_name"),
    specRevision: optionalInteger(data, "spec_revision"),
    provenance:
      data.provenance !== undefined && data.provenance !== null
        ? parseAgentProvenance(data.provenance)
        : null,
    openOperations: strings(data.open_operations),
    addressability: optionalText(data, "addressability"),
    runtimeModeOverride: optionalText(data, "runtime_mode_override"),
    rosterListingSuperseded: data.roster_listing_superseded === true,
  };
}

/** @internal */
export function parseAgentSummary(raw: unknown): AgentSummary {
  return parseAgentSummaryOf(object(raw, "an agent"), "an agent");
}

/**
 * One agent in full: what a list says of it, and the rest of what a create
 * or an update may set, under the names they set it by.
 */
export interface AgentRecord extends AgentSummary {
  readonly instructions: readonly string[];
  readonly context: unknown;
  readonly labels: Readonly<Record<string, string>>;
  readonly peers: readonly string[];
}

/** @internal */
export function parseAgentRecord(raw: unknown): AgentRecord {
  const what = "an agent record";
  const data = object(raw, what);
  const labels: Record<string, string> = {};
  if (typeof data.labels === "object" && data.labels !== null && !Array.isArray(data.labels)) {
    for (const [key, value] of Object.entries(data.labels as Record<string, unknown>)) {
      if (typeof value === "string") labels[key] = value;
    }
  }
  return {
    ...parseAgentSummaryOf(data, what),
    instructions: strings(data.instructions),
    context: data.context ?? null,
    labels,
    peers: strings(data.peers),
  };
}

/**
 * A page of the mob's agents, in one stable order. `next` is the name to
 * read the next page after (`list({ after: page.next })`), which continues
 * that order; `null` on the last page. Do not rely on the order being
 * alphabetical.
 */
export interface AgentPage {
  readonly agents: readonly AgentSummary[];
  readonly next: string | null;
}

/** @internal */
export function parseAgentPage(raw: unknown): AgentPage {
  const data = object(raw, "an agent page");
  if (!Array.isArray(data.agents)) {
    throw new ContractMismatchError("an agent page has no agents");
  }
  return { agents: data.agents.map(parseAgentSummary), next: optionalText(data, "next") };
}

/**
 * One profile of the directory, with what the host's rules give a create of
 * an agent of it. `verdict` is advisory (today `allowed`; `needs_decision`,
 * with the `lines` the decision shows; `not_allowed`, with the rule's
 * `reason` when it gives one).
 */
export interface DirectoryProfile {
  readonly profile: string;
  readonly verdict: string;
  readonly lines: readonly string[];
  readonly reason: string | null;
}

/**
 * One agent of the directory, with what the host's rules give listing it as
 * a peer: `verdict`, `lines` and `reason` as on a {@link DirectoryProfile}.
 * `rosterListingSuperseded` is as on an {@link AgentSummary}.
 */
export interface DirectoryPeer {
  readonly name: string;
  readonly profile: string;
  readonly displayName: string | null;
  readonly verdict: string;
  readonly lines: readonly string[];
  readonly reason: string | null;
  readonly rosterListingSuperseded?: boolean;
}

/**
 * A page of what the caller may name in a create or an update: the mob's
 * profiles, and the agents it may list as peers. `next` is the name to read
 * the next page after; `null` on the last page.
 */
export interface AgentDirectoryPage {
  readonly profiles: readonly DirectoryProfile[];
  readonly peers: readonly DirectoryPeer[];
  readonly next: string | null;
}

/** @internal */
export function parseAgentDirectoryPage(raw: unknown): AgentDirectoryPage {
  const data = object(raw, "a directory page");
  const profiles = Array.isArray(data.profiles) ? data.profiles : [];
  const peers = Array.isArray(data.peers) ? data.peers : [];
  return {
    profiles: profiles.map((entry) => {
      const what = "a directory profile";
      const profile = object(entry, what);
      return {
        profile: text(profile, "profile", what),
        verdict: text(profile, "verdict", what),
        lines: strings(profile.lines),
        reason: optionalText(profile, "reason"),
      };
    }),
    peers: peers.map((entry) => {
      const what = "a directory peer";
      const peer = object(entry, what);
      return {
        name: text(peer, "name", what),
        profile: text(peer, "profile", what),
        displayName: optionalText(peer, "display_name"),
        verdict: text(peer, "verdict", what),
        lines: strings(peer.lines),
        reason: optionalText(peer, "reason"),
        rosterListingSuperseded: peer.roster_listing_superseded === true,
      };
    }),
    next: optionalText(data, "next"),
  };
}

/**
 * The plan of a change: the outcome and the rendering it would get. Nothing
 * is held and nothing is written.
 *
 * `outcome` is what the host's rules give it (advisory; today `allow` and
 * `require_approval`); a change they deny is refused, not planned. `change`
 * is the change class whose rule decided (advisory). `approverId` is who
 * decides, when the outcome names one. `subjectDigest` is the digest of
 * `rendering`, the text a decider would be shown, whole: it is what a
 * decision would bind. `schedules` is on the plan of an archive, an
 * unarchive and a removal: the schedules the change would take with it
 * ({@link SchedulesFollowed}), as they are when the plan is made; it is no
 * part of the rendering. `rosterListingSuperseded` is `true` when the
 * host's roster lists the agent the change names while the agent is served
 * from its record, so the change is of the record alone; the gateway writes
 * it only when it is so, and a parsed plan always carries it.
 */
export interface IdentityChangePlan {
  readonly outcome: string;
  readonly change: string;
  readonly approverId: string | null;
  readonly timeoutMs: number;
  readonly requestDigest: string;
  readonly subjectDigest: string;
  readonly rendering: string;
  readonly schedules: SchedulesFollowed | null;
  readonly rosterListingSuperseded?: boolean;
}

/** @internal */
export function parseIdentityChangePlan(raw: unknown): IdentityChangePlan {
  const what = "a change plan";
  const data = object(raw, what);
  return {
    outcome: text(data, "outcome", what),
    change: text(data, "change", what),
    approverId: optionalText(data, "approver_id"),
    timeoutMs: integer(data, "timeout_ms", what),
    requestDigest: text(data, "request_digest", what),
    subjectDigest: text(data, "subject_digest", what),
    rendering: text(data, "rendering", what),
    schedules: parseSchedulesFollowed(data),
    rosterListingSuperseded: data.roster_listing_superseded === true,
  };
}

/** A withdrawn change: its decision ended, and nothing was written. */
export interface IdentityWithdrawn {
  readonly operationId: string;
  readonly decisionId: string;
}

/** @internal */
export function parseIdentityWithdrawn(raw: unknown): IdentityWithdrawn {
  const what = "a withdrawal";
  const data = object(raw, what);
  return {
    operationId: text(data, "operation_id", what),
    decisionId: text(data, "decision_id", what),
  };
}

/** Whether the mob's changes of agents are held now. */
export interface IdentityChangesHold {
  readonly held: boolean;
}

/** @internal */
export function parseIdentityChangesHold(raw: unknown): IdentityChangesHold {
  const data = object(raw, "a changes hold");
  if (typeof data.held !== "boolean") {
    throw new ContractMismatchError("a changes hold has no held");
  }
  return { held: data.held };
}
