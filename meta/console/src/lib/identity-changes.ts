// Changes of the mob's long-term agents, as the console sends and reads
// them: the `meta/identity/*` family (meerkat-meta
// identity_creation/family.rs and outcome.rs). The console owns no meaning
// here. It writes the request a form describes, reads the typed outcome and
// the typed refusal, and says both in words; who may do what is the
// gateway's answer (the experience read's `identity_changes` section and
// each agent's two affordances), never the console's.

import type { ConsoleAgentAffordances, ConsoleExperience } from "../types";
import { agentDisplayTitle } from "../../../packages/console-core/src/agent-title";

/** The JSON-RPC code of every refusal of the family. */
export const IDENTITY_REFUSED_CODE = -32058;

/**
 * An agent as people read it: its display name, or, where its record has
 * none, its name in words, as the roster names it. The name itself stays a
 * tooltip.
 */
export function agentShownName(agent: { name: string; displayName?: string }): string {
  return agent.displayName?.trim()
    || agentDisplayTitle({ label: "", identity: agent.name, member_id: agent.name, agent_id: agent.name });
}

// ---------------------------------------------------------------------------
// What the gateway lets this caller do
// ---------------------------------------------------------------------------

export interface IdentityChangeFlags {
  /** The console serves changes of agents to this caller at all. */
  available: boolean;
  canCreate: boolean;
  canChange: boolean;
  canRemove: boolean;
}

/** The experience read's `identity_changes` section. A member that is not
 * `true` is off: an older gateway, which sends none, offers nothing. */
export function identityChangeFlags(experience: Pick<ConsoleExperience, "identity_changes"> | null | undefined): IdentityChangeFlags {
  const section = experience?.identity_changes;
  const available = section?.available === true;
  return {
    available,
    canCreate: available && section?.can_create === true,
    canChange: available && section?.can_change === true,
    canRemove: available && section?.can_remove === true,
  };
}

export interface AgentIdentityControls {
  edit: boolean;
  archive: boolean;
  remove: boolean;
}

/** The controls of one agent of the roster: the section's flag and the
 * agent's own affordance, both the gateway's. */
export function agentIdentityControls(
  flags: IdentityChangeFlags,
  affordances: Pick<ConsoleAgentAffordances, "can_change_identity" | "can_remove_identity"> | null | undefined,
): AgentIdentityControls {
  const change = flags.canChange && affordances?.can_change_identity === true;
  return {
    edit: change,
    archive: change,
    remove: flags.canRemove && affordances?.can_remove_identity === true,
  };
}

/** The controls of an archived agent. It has no member, so the roster read
 * carries no row and no affordance for it: the section's flags offer the
 * controls, and the gateway judges the agent when the change is sent. */
export function archivedAgentControls(flags: IdentityChangeFlags): { unarchive: boolean; remove: boolean } {
  return { unarchive: flags.canChange, remove: flags.canRemove };
}

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

type Raw = Record<string, unknown>;

function rawOf(value: unknown): Raw | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as Raw : null;
}

function textOf(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function textsOf(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
}

function numberOf(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** What the host's rules give one entry of the directory: `allowed`,
 * `needs_decision` (with the lines a decision shows) or `not_allowed` (with
 * the rule's reason). Advisory: another value is kept as it is. */
export interface DirectoryVerdict {
  verdict: string;
  lines: string[];
  reason?: string;
}

export interface DirectoryProfile extends DirectoryVerdict {
  profile: string;
}

export interface DirectoryPeer extends DirectoryVerdict {
  name: string;
  displayName?: string;
  profile: string;
}

export interface IdentityDirectoryPage {
  profiles: DirectoryProfile[];
  peers: DirectoryPeer[];
  next?: string;
}

function verdictOf(entry: Raw): DirectoryVerdict {
  const reason = textOf(entry.reason);
  return {
    verdict: textOf(entry.verdict) ?? "",
    lines: textsOf(entry.lines),
    ...(reason ? { reason } : {}),
  };
}

/** A page of `meta/identity/directory`. */
export function parseIdentityDirectory(value: unknown): IdentityDirectoryPage {
  const page = rawOf(value) ?? {};
  const profiles = (Array.isArray(page.profiles) ? page.profiles : []).flatMap((entry): DirectoryProfile[] => {
    const raw = rawOf(entry);
    const profile = textOf(raw?.profile);
    return raw && profile ? [{ profile, ...verdictOf(raw) }] : [];
  });
  const peers = (Array.isArray(page.peers) ? page.peers : []).flatMap((entry): DirectoryPeer[] => {
    const raw = rawOf(entry);
    const name = textOf(raw?.name);
    if (!raw || !name) return [];
    const displayName = textOf(raw.display_name);
    return [{ name, ...(displayName ? { displayName } : {}), profile: textOf(raw.profile) ?? "", ...verdictOf(raw) }];
  });
  const next = textOf(page.next);
  return { profiles, peers, ...(next ? { next } : {}) };
}

/** How, when and at whose request an agent came to exist, as its record
 * says. How it entered is on the wire and is not kept: the console shows no
 * origin. */
export interface AgentProvenance {
  /** `agent`, `host` or `human`. */
  requesterKind: string;
  /** An agent's name, a host surface, or a person's principal key. */
  requester: string;
  atMs: number;
  /** The consent's form (`allowed`, `decided`, `imported`). Advisory: a
   * form the console has no words for reads as it is. */
  consentForm: string;
}

export interface AgentSummary {
  name: string;
  displayName?: string;
  profile: string;
  specDigest: string;
  specRevision?: number;
  archived: boolean;
  /** Where the agent's member stands. Advisory. */
  readiness: string;
  provenance?: AgentProvenance;
  openOperations: string[];
}

export interface AgentRecord extends AgentSummary {
  instructions: string[];
  context?: unknown;
  labels: Record<string, string>;
  peers: string[];
}

function provenanceOf(value: unknown): AgentProvenance | undefined {
  const raw = rawOf(value);
  const requester = textOf(raw?.requester);
  const atMs = numberOf(raw?.at_ms);
  if (!raw || !requester || atMs === undefined) return undefined;
  return {
    requesterKind: textOf(raw.requester_kind) ?? "",
    requester,
    atMs,
    consentForm: textOf(rawOf(raw.consent)?.form) ?? "",
  };
}

function summaryOf(raw: Raw): AgentSummary | null {
  const name = textOf(raw.name);
  const specDigest = textOf(raw.spec_digest);
  if (!name || !specDigest) return null;
  const displayName = textOf(raw.display_name);
  const specRevision = numberOf(raw.spec_revision);
  const provenance = provenanceOf(raw.provenance);
  return {
    name,
    ...(displayName ? { displayName } : {}),
    profile: textOf(raw.profile) ?? "",
    specDigest,
    ...(specRevision !== undefined ? { specRevision } : {}),
    archived: raw.archived === true,
    readiness: textOf(raw.readiness) ?? "",
    ...(provenance ? { provenance } : {}),
    openOperations: textsOf(raw.open_operations),
  };
}

/** The answer of `meta/identity/get`; null when it is no agent's record. */
export function parseAgentRecord(value: unknown): AgentRecord | null {
  const raw = rawOf(value);
  const summary = raw ? summaryOf(raw) : null;
  if (!raw || !summary) return null;
  const labels: Record<string, string> = {};
  for (const [key, label] of Object.entries(rawOf(raw.labels) ?? {})) {
    if (typeof label === "string") labels[key] = label;
  }
  return {
    ...summary,
    instructions: textsOf(raw.instructions),
    ...(raw.context !== undefined && raw.context !== null ? { context: raw.context } : {}),
    labels,
    peers: textsOf(raw.peers),
  };
}

/** A page of `meta/identity/list`. */
export function parseAgentPage(value: unknown): { agents: AgentSummary[]; next?: string } {
  const page = rawOf(value) ?? {};
  const agents = (Array.isArray(page.agents) ? page.agents : []).flatMap((entry): AgentSummary[] => {
    const raw = rawOf(entry);
    const summary = raw ? summaryOf(raw) : null;
    return summary ? [summary] : [];
  });
  const next = textOf(page.next);
  return { agents, ...(next ? { next } : {}) };
}

/** Every page of a paged read: `read(after)` answers one page and the name
 * to read the next after. A page that names the cursor it was read after
 * ends the walk instead of repeating it. */
export async function readAllPages<T>(read: (after: string | undefined) => Promise<{ items: T[]; next?: string }>): Promise<T[]> {
  const all: T[] = [];
  const seen = new Set<string>();
  let after: string | undefined;
  for (;;) {
    const page = await read(after);
    all.push(...page.items);
    if (!page.next || seen.has(page.next)) return all;
    seen.add(page.next);
    after = page.next;
  }
}

// ---------------------------------------------------------------------------
// Outcomes and refusals
// ---------------------------------------------------------------------------

/** What became of an update's instructions change. */
export interface InstructionOutcome {
  kind: string;
  reason?: string;
}

export interface AgentOutcome {
  name: string;
  /** `committed`, `pending_preparation`, `archiving`, `archived`,
   * `restored`, `retiring` or `removed`. */
  state: string;
  specDigest?: string;
  specRevision?: number;
  /** `seated` or `owed`, on a create. */
  seat?: string;
  instructions?: InstructionOutcome;
}

export type IdentityOutcome =
  | { outcome: "done" | "pending_preparation"; operationId: string; agents: AgentOutcome[] }
  | { outcome: "pending_decision"; operationId: string; decisionId: string; deadlineMs: number }
  /** The operation read's answer for an operation nothing committed under
   * and no decision is held for. */
  | { outcome: "unknown"; operationId: string }
  /** The operation read's answer for a refused operation. */
  | { outcome: "refused"; refusal: IdentityRefusal };

/** One refusal of the family: a kind of the closed vocabulary, the field
 * of the request it concerns (a path into the request as written), whether
 * the same request may be sent again, and a sentence to act on. */
export interface IdentityRefusal {
  kind: string;
  field?: string;
  retryable: boolean;
  detail: string;
  /** The kind's own members (`current_digest`, `by`, `profile`, ...). */
  data: Raw;
  /**
   * Absent for the family's own refusal. `gateway`: another answer of the
   * gateway that refuses the call (access denied, a read-only console, a
   * candidate launch, any other error it answered). `console`: the console
   * itself sent nothing (the gateway does not offer the method, or the
   * console does not know whom it acts for).
   */
  origin?: "gateway" | "console";
}

function refusalOf(data: unknown, message?: unknown): IdentityRefusal | null {
  const raw = rawOf(data);
  const kind = textOf(raw?.kind);
  if (!raw || !kind) return null;
  const field = textOf(raw.field);
  return {
    kind,
    ...(field ? { field } : {}),
    retryable: raw.retryable === true,
    detail: textOf(raw.detail)?.trim() || textOf(message)?.trim() || "",
    data: raw,
  };
}

function agentOutcomeOf(value: unknown): AgentOutcome | null {
  const raw = rawOf(value);
  const name = textOf(raw?.name);
  if (!raw || !name) return null;
  const specDigest = textOf(raw.spec_digest);
  const specRevision = numberOf(raw.spec_revision);
  const seat = textOf(raw.seat);
  const instructions = rawOf(raw.instructions);
  const instructionKind = textOf(instructions?.kind);
  const reason = textOf(instructions?.reason);
  return {
    name,
    state: textOf(raw.state) ?? "",
    ...(specDigest ? { specDigest } : {}),
    ...(specRevision !== undefined ? { specRevision } : {}),
    ...(seat ? { seat } : {}),
    ...(instructionKind ? { instructions: { kind: instructionKind, ...(reason ? { reason } : {}) } } : {}),
  };
}

/** The answer of a change or of the operation read; null for an answer
 * that is no outcome of the family. */
export function parseIdentityOutcome(value: unknown): IdentityOutcome | null {
  const raw = rawOf(value);
  const operationId = textOf(raw?.operation_id) ?? "";
  switch (raw?.outcome) {
    case "done":
    case "pending_preparation":
      return {
        outcome: raw.outcome,
        operationId,
        agents: (Array.isArray(raw.agents) ? raw.agents : []).flatMap((entry) => {
          const agent = agentOutcomeOf(entry);
          return agent ? [agent] : [];
        }),
      };
    case "pending_decision": {
      const decisionId = textOf(raw.decision_id);
      const deadlineMs = numberOf(raw.deadline_ms);
      return decisionId && deadlineMs !== undefined ? { outcome: "pending_decision", operationId, decisionId, deadlineMs } : null;
    }
    case "unknown":
      return { outcome: "unknown", operationId };
    case "refused": {
      const refusal = refusalOf(raw);
      return refusal ? { outcome: "refused", refusal } : null;
    }
    default:
      return null;
  }
}

/** What the gateway says a change would get (`meta/identity/plan`): the
 * outcome its host's rules give it. Nothing is held and nothing is written
 * by asking. */
export interface ChangePlan {
  /** `allow`, `notify` or `require_approval`. Advisory: another value is
   * kept as it is. */
  outcome: string;
}

/** The answer of `meta/identity/plan`; null when it is no plan. */
export function parseChangePlan(value: unknown): ChangePlan | null {
  const outcome = textOf(rawOf(value)?.outcome);
  return outcome ? { outcome } : null;
}

/** A plan in words, for the field of the form whose change was planned.
 * Null for an outcome this console has no words for: it then says nothing,
 * and the gateway judges the change when it is sent. */
export function planWords(plan: ChangePlan): { tone: NoticeTone; label: string; line: string } | null {
  switch (plan.outcome) {
    case "allow":
    // A change its host is told about is made like an allowed one.
    case "notify":
      return { tone: "ok", label: "Allowed", line: "The host's rules allow this change." };
    case "require_approval":
      return { tone: "warn", label: "Needs a decision", line: "A person must decide this change before it is made." };
    default:
      return null;
  }
}

/** The refusal an error of the console transport carries: the family's one
 * JSON-RPC error. Null for any other failure. */
export function identityRefusalOf(error: unknown): IdentityRefusal | null {
  const rpc = (error as { rpcError?: { code?: unknown; message?: unknown; data?: unknown } } | null)?.rpcError;
  if (rpc?.code !== IDENTITY_REFUSED_CODE) return null;
  return refusalOf(rpc.data, rpc.message);
}

/**
 * What a failed call was, when it was an answer: the family's refusal, any
 * other error the gateway answered (the call was refused and nothing was
 * made), or the console's own refusal to send (the gateway does not offer
 * the method; the console does not know whom it acts for). Null only when
 * no answer came: a timeout, a lost connection, an answer that cannot be
 * read. Only then may the change have been made.
 */
export function answeredRefusalOf(error: unknown): IdentityRefusal | null {
  const own = identityRefusalOf(error);
  if (own) return own;
  const failure = error as {
    rpcError?: { message?: unknown; data?: unknown };
    responseRpcError?: { message?: unknown; data?: unknown };
    kind?: unknown;
    transportFailure?: unknown;
    message?: unknown;
  } | null;
  const rpc = failure?.rpcError ?? failure?.responseRpcError;
  if (rpc) {
    const data = rawOf(rpc.data) ?? {};
    return { kind: textOf(data.kind) ?? "", retryable: false, detail: textOf(rpc.message)?.trim() ?? "", data, origin: "gateway" };
  }
  if (failure?.kind === "console-capability-unavailable") {
    return { kind: "not_offered", retryable: false, detail: "", data: {}, origin: "console" };
  }
  if (failure?.transportFailure === "caller_unknown") {
    return { kind: "caller_unknown", retryable: false, detail: textOf(failure.message) ?? "", data: {}, origin: "console" };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/** The change a form or a confirmation sends. */
export type IdentityChangeKind = "create" | "update" | "archive" | "unarchive" | "remove";

export interface NewAgentDraft {
  profile: string;
  name: string;
  displayName: string;
  /** One text area: one section. */
  instructions: string;
  firstMessage: string;
  peers: string[];
}

export const EMPTY_NEW_AGENT_DRAFT: NewAgentDraft = {
  profile: "", name: "", displayName: "", instructions: "", firstMessage: "", peers: [],
};

/** The create request of one agent, without its `request_id`. A member the
 * form left empty is left out. */
export function createAgentsRequest(draft: NewAgentDraft): Raw {
  const displayName = draft.displayName.trim();
  return {
    agents: [{
      name: draft.name.trim(),
      profile: draft.profile,
      ...(displayName ? { display_name: displayName } : {}),
      ...(draft.instructions.trim() ? { instructions: [draft.instructions] } : {}),
      ...(draft.firstMessage.trim() ? { first_message: draft.firstMessage } : {}),
      ...(draft.peers.length ? { peers: [...draft.peers] } : {}),
    }],
  };
}

export interface AgentLabelDraft {
  key: string;
  value: string;
}

export interface AgentEditDraft {
  displayName: string;
  /** One text area per section of the agent's instructions. */
  instructions: string[];
  /** The context as JSON text; empty for none. */
  context: string;
  labels: AgentLabelDraft[];
  profile: string;
  peers: string[];
}

/** The form of an agent as its record reads now. An agent without a
 * display name opens with an empty one. */
export function editDraftOf(record: AgentRecord): AgentEditDraft {
  return {
    displayName: record.displayName ?? "",
    instructions: record.instructions.length ? [...record.instructions] : [""],
    context: record.context === undefined ? "" : JSON.stringify(record.context, null, 2),
    labels: Object.entries(record.labels).map(([key, value]) => ({ key, value })),
    profile: record.profile,
    peers: [...record.peers],
  };
}

function sameJson(left: unknown, right: unknown): boolean {
  return canonicalJson(left) === canonicalJson(right);
}

export type UpdateAgentRequest =
  /** The update, without its `request_id`; `changed` is false when the
   * form sets nothing the record does not hold already. */
  | { request: Raw; changed: boolean }
  /** The context is no JSON value, so no request can carry it. */
  | { contextError: string };

/** The update request of an agent: only what the form changed, against the
 * record it was opened on, at that record's spec digest. */
export function updateAgentRequest(record: AgentRecord, draft: AgentEditDraft): UpdateAgentRequest {
  const set: Raw = {};
  // A display name the form emptied is sent as it is: the gateway says
  // what is wrong with it, at its field.
  const displayName = draft.displayName.trim();
  if (displayName !== (record.displayName ?? "")) set.display_name = displayName;
  const instructions = draft.instructions.filter((section) => section.trim().length > 0);
  if (!sameJson(instructions, record.instructions)) set.instructions = instructions;
  const contextText = draft.context.trim();
  if (contextText) {
    let context: unknown;
    try {
      context = JSON.parse(contextText);
    } catch {
      return { contextError: "The context is not valid JSON." };
    }
    if (!sameJson(context, record.context)) set.context = context;
  } else if (record.context !== undefined) {
    // An update's absent context is unchanged, so no request takes one away.
    return { contextError: "A context can be replaced, not removed." };
  }
  const labels: Record<string, string> = {};
  for (const { key, value } of draft.labels) {
    if (key.trim()) labels[key.trim()] = value;
  }
  const labelsSet: Record<string, string> = {};
  for (const [key, value] of Object.entries(labels)) {
    if (record.labels[key] !== value) labelsSet[key] = value;
  }
  const labelsRemove = Object.keys(record.labels).filter((key) => !Object.hasOwn(labels, key));
  if (Object.keys(labelsSet).length) set.labels = labelsSet;
  if (labelsRemove.length) set.labels_remove = labelsRemove;
  if (draft.profile && draft.profile !== record.profile) set.profile = draft.profile;
  const peersAdd = draft.peers.filter((peer) => !record.peers.includes(peer));
  const peersRemove = record.peers.filter((peer) => !draft.peers.includes(peer));
  return {
    request: {
      name: record.name,
      expected_spec_digest: record.specDigest,
      set,
      ...(peersAdd.length ? { peers_add: peersAdd } : {}),
      ...(peersRemove.length ? { peers_remove: peersRemove } : {}),
    },
    changed: Object.keys(set).length > 0 || peersAdd.length > 0 || peersRemove.length > 0,
  };
}

/** The request of a change of one agent as a whole (archive, unarchive,
 * remove), at the spec digest its record reads, without its `request_id`. */
export function agentRefRequest(record: Pick<AgentSummary, "name" | "specDigest">): Raw {
  return { name: record.name, expected_spec_digest: record.specDigest };
}

/** JSON with its object keys in order: equal values write equal text. */
export function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
  if (value !== null && typeof value === "object") {
    const record = value as Raw;
    return `{${Object.keys(record).sort().map((key) => `${JSON.stringify(key)}:${canonicalJson(record[key])}`).join(",")}}`;
  }
  return JSON.stringify(value) ?? "null";
}

/** The `request_id` of one action and the request it was made for. */
export interface RequestIdSlot {
  requestId: string;
  fingerprint: string;
}

/**
 * The `request_id` a change is sent under. One action sends the same
 * request under the same id however often it is sent again (after a lost
 * answer, a refusal that may be retried, or Retry), so the gateway answers
 * the same state and writes nothing twice. A request the person changed is
 * another request and takes a new id: the gateway refuses another request
 * under a used id.
 */
export function requestIdFor(slot: RequestIdSlot | null, change: IdentityChangeKind, request: Raw, mint: () => string): RequestIdSlot {
  const fingerprint = `${change}:${canonicalJson(request)}`;
  return slot && slot.fingerprint === fingerprint ? slot : { requestId: mint(), fingerprint };
}

// ---------------------------------------------------------------------------
// Where a refusal belongs in a form
// ---------------------------------------------------------------------------

export type IdentityFormField =
  | "name"
  | "profile"
  | "display_name"
  | "instructions"
  | "first_message"
  | "peers"
  | "context"
  | "labels";

const CREATE_FIELDS: Readonly<Record<string, IdentityFormField>> = {
  name: "name",
  profile: "profile",
  display_name: "display_name",
  instructions: "instructions",
  first_message: "first_message",
  peers: "peers",
};

const UPDATE_FIELDS: Readonly<Record<string, IdentityFormField>> = {
  "set.display_name": "display_name",
  "set.instructions": "instructions",
  "set.context": "context",
  "set.labels": "labels",
  "set.labels_remove": "labels",
  "set.profile": "profile",
  peers_add: "peers",
  peers_remove: "peers",
};

/** The form field a refusal's `field` names: a path into the request the
 * form wrote. Null for a path the form has no field for (the request id,
 * the spec digest, the agent a change names): the refusal then shows as
 * the form's own notice. */
export function refusalFormField(change: IdentityChangeKind, path: string | undefined): IdentityFormField | null {
  if (!path) return null;
  // An element of a list belongs to the list's field.
  const member = path.replace(/\[\d+\]$/, "");
  if (change === "create") {
    // The console's create names one agent.
    const own = /^agents\[0\]\.(.+)$/.exec(member);
    return own ? CREATE_FIELDS[own[1]] ?? null : null;
  }
  if (change === "update") return UPDATE_FIELDS[member] ?? null;
  return null;
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

export type NoticeTone = "ok" | "warn" | "crit" | "neutral";

const NEXT_SESSION_REASONS: Readonly<Record<string, string>> = {
  prepared_authorization_required: "this team's runtime is governed, and the change needs an authorization its host prepares",
  slot_limit_exceeded: "the team holds as many instruction records as it can",
  active_set_budget_exceeded: "the team's instruction records are as large together as they can be",
  lowering_unavailable: "the agent's model cannot be given the change while it runs",
  body_too_large: "the instructions are larger than one instruction record holds",
  record_unavailable: "the team could not record the change now, and records it at its next start",
};

/** What became of an instructions change, in words. Null where the update
 * carried none. */
export function instructionOutcomeWords(outcome: InstructionOutcome | undefined): string | null {
  switch (outcome?.kind) {
    case undefined:
      return null;
    case "applies_at_next_run":
      return "The new instructions reach the agent at its next run. A run that starts within moments of the change may still use the earlier instructions, and earlier instructions are never rewritten.";
    case "applies_after_compaction":
      return "The new instructions reach the agent after its next compaction: its model takes new instructions only at the start of what it reads. Earlier instructions are never rewritten.";
    case "applies_at_next_session": {
      const reason = outcome.reason ? NEXT_SESSION_REASONS[outcome.reason] : undefined;
      return reason
        ? `The new instructions reach the agent at its next new conversation: ${reason}.`
        : "The new instructions reach the agent at its next new conversation.";
    }
    case "unchanged":
      return "The instructions are unchanged.";
    default:
      return "The new instructions are in the agent's record.";
  }
}

/** A create's seat, in words. Null for a seated agent: nothing to add. */
export function seatWords(seat: string | undefined): string | null {
  return seat === "owed"
    ? "It exists and its name is taken, but it has not started yet: its first message did not start. It starts with its next message, and its first message runs once then."
    : null;
}

export interface OutcomeWords {
  tone: NoticeTone;
  /** The headline: what happened, naming the agent as people know it. */
  title: string;
  /** What follows from it. */
  lines: string[];
}

/** A change's outcome in words. `name` is the agent as people know it (its
 * display name where it has one). */
export function outcomeWords(change: IdentityChangeKind, outcome: IdentityOutcome, name: string): OutcomeWords {
  if (outcome.outcome === "pending_decision") {
    return {
      tone: "warn",
      title: "A person must decide this change",
      lines: [
        "Nothing has changed yet. The request waits for a decision, and you cannot approve your own request.",
      ],
    };
  }
  if (outcome.outcome === "unknown") {
    return { tone: "neutral", title: "The request is no longer open", lines: ["It was withdrawn or refused, and nothing changed."] };
  }
  if (outcome.outcome === "refused") {
    return { tone: "crit", title: refusalSentence(outcome.refusal), lines: [] };
  }
  const agent = outcome.agents[0];
  const state = agent?.state ?? "";
  if (outcome.outcome === "pending_preparation" || state === "pending_preparation") {
    return {
      tone: "warn",
      title: `${name} is being prepared`,
      lines: ["The change is made. The agent cannot be used until its preparation ends."],
    };
  }
  const lines: string[] = [];
  let title: string;
  switch (state) {
    case "archiving":
      title = `${name} is stopping`;
      lines.push("It is archived when that ends.");
      break;
    case "archived":
      title = `${name} is archived`;
      lines.push("It keeps its name, its record, its conversation and its memory. Messages to it are refused until it is unarchived.");
      break;
    case "restored":
      title = `${name} is back`;
      lines.push("It continues the conversation it had.");
      break;
    case "retiring":
      title = `${name} is stopping`;
      lines.push("It is removed when that ends. Its name can never be used again.");
      break;
    case "removed":
      title = `${name} is removed`;
      lines.push("Its name can never be used again.");
      break;
    default:
      if (change === "create") {
        title = `${name} is created`;
        const seat = seatWords(agent?.seat);
        // What a new agent does at its start is its profile's: the console
        // promises neither a turn nor idleness.
        lines.push(seat ?? "It is a member of the team now.");
      } else {
        title = `${name} is updated`;
      }
  }
  const instructions = instructionOutcomeWords(agent?.instructions);
  if (instructions) lines.push(instructions);
  return { tone: "ok", title, lines };
}

/**
 * The console's own sentence for each kind of refusal: what happened and
 * what the person can do, in the console's words. The gateway's sentences
 * are written for every caller of the family, a model included, and speak
 * of what a person at the console never sees; they are kept for Details.
 *
 * The first block is the family's closed vocabulary, in its own order; the
 * second the gateway's other answers and the console's own refusals to
 * send.
 */
const REFUSAL_SENTENCES: Readonly<Record<string, string>> = {
  profile_unknown: "This team has no such profile. Pick one of the team's profiles.",
  profile_not_allowed: "This profile is not one you may use here. Pick another profile.",
  privileged_field: "This change sets something you may not set. Leave it out.",
  grant_revoked: "You are no longer allowed to make this change. It was not made.",
  reserved_key: "This label key is reserved by the host. Use another key.",
  authority_field_needs_approval: "This label is one that only a person's decision sets. Leave it out.",
  authority_field_invalid: "This label, or its value, is not one the host allows.",
  text_too_large: "This text is too long. Shorten it; text is never cut.",
  consequences_unavailable: "The host could not say what this change would lead to. Try again later.",
  invalid_request: "Part of this change is missing, contradicts another part or is not valid. Correct it and try again.",
  invalid_name: "This is not a valid agent name. Choose another.",
  name_taken: "This name is taken. Choose another; a removed agent's name is never free again.",
  peer_not_allowed: "The host's rules do not allow this peer. Leave it out.",
  peer_unknown: "This peer is not an agent of this team that you can name. Pick another agent.",
  batch_too_large: "This names more agents than one change can make. Make fewer at a time.",
  validation_refused: "The host's own check refused this change.",
  change_denied: "The host's rules do not allow you this change. It was not made, and asking again changes nothing.",
  unsupported: "This change needs something this version does not support. Leave it out.",
  identity_unknown: "This team has no agent of this name. It may have been removed.",
  self_change_refused: "An agent cannot change itself.",
  private_requester: "A private chat cannot ask for changes of agents. Nothing was made.",
  stale_spec: "This agent changed since you opened it. Read it again, then make your change.",
  operation_reused: "The team has already seen another change sent this way. Close this and start the change again.",
  owner_read_only: "This agent is set up in the host's configuration and cannot be changed here. Change it in the configuration.",
  not_adoptable: "This agent cannot be released from the host's configuration now.",
  not_archived: "This agent is not archived, so there is nothing to bring back.",
  declared_apply_open: "The host is applying its configuration. Try again shortly.",
  admission_unsettled: "This agent is in the middle of another step. Try again once it has finished.",
  predecessor_mismatch: "This agent is not as it was when the change was asked for. Read it again.",
  import_refused: "Only the host itself can carry agents in from outside.",
  purge_refused: "The host refused to purge the team's records.",
  preparation_not_pending: "The host is not preparing this agent.",
  preparation_mismatch: "The host's preparation of this agent belongs to another change.",
  construction_open: "The host is still setting this agent up.",
  stale_attempt: "The host's setup of this agent has moved on since this was asked.",
  not_constructed: "The host has not started setting this agent up.",
  construction_required: "The host must finish setting this agent up first.",
  activation_pending: "This agent is being started, and nothing can replace that now.",
  approval_unavailable: "This change needs a person's decision, and this team has nobody who can be asked. Nothing was made.",
  decision_open: "A decision about this agent is already open. Wait for its outcome.",
  subject_widened: "The change is no longer what was approved. Ask again.",
  decision_closed: "The decision has ended. Ask again if the change is still wanted.",
  decision_not_found: "There is no open request to act on. It may have been decided or withdrawn already.",
  evidence_required: "The decision was not recorded as a person's own. Decide it again from the console.",
  subject_changed: "What was shown is not what the decision is about. Open the decision again.",
  presentation_incomplete: "The decision was not shown in full. Open the decision again.",
  requester_cannot_decide: "A person cannot approve their own request.",
  approver_not_eligible: "You are not the person who decides this request.",
  decided_before_presentation: "The decision was answered before it was shown. Open the decision again.",
  changes_held: "The host has paused changes of agents for now. Try again later.",
  host_preparation_pending: "The host is still preparing this agent. Try again later.",
  candidate_launch: "This is a trial launch of the team, and a trial launch changes nothing.",
  identity_member: "This is a long-term agent, and this action is for short-lived helpers.",
  storage_unsupported: "This team's storage cannot make changes of agents.",
  mob_not_running: "The team is not running.",
  purged: "The team's records were cleared before a downgrade, and changes are refused until the team restarts.",
  construction_refused: "The host could not set this agent up.",

  access_denied: "You do not have access to this. Nothing was changed.",
  read_only: "This console is read-only. Nothing was changed.",
  no_decision_store: "This team keeps no decisions, so there is no request to act on.",
  not_offered: "This team does not offer this here. Nothing was sent.",
  caller_unknown: "The console does not know who you are signed in as, so it sent nothing. Reload the page and sign in again.",
};

/** The kinds the console has a sentence of its own for. */
export const REFUSAL_KINDS_WITH_A_SENTENCE: readonly string[] = Object.keys(REFUSAL_SENTENCES);

/** The sentence of a refusal whose kind the console does not know. */
export const UNKNOWN_REFUSAL_SENTENCE = "The request was refused, and nothing was changed.";

/** The sentence of a refusal: the console's own for its kind, the plain one
 * for a kind the console does not know. Never the kind token, and never the
 * gateway's sentence: that one is the refusal's detail. */
export function refusalSentence(refusal: IdentityRefusal): string {
  return Object.hasOwn(REFUSAL_SENTENCES, refusal.kind) ? REFUSAL_SENTENCES[refusal.kind] : UNKNOWN_REFUSAL_SENTENCE;
}

/** What a refusal's Details disclosure holds: the gateway's own sentence,
 * whole. Null where the refusal brings none. */
export function refusalDetail(refusal: IdentityRefusal): string | null {
  return refusal.detail || null;
}

export interface VerdictWords {
  label: string;
  tone: NoticeTone;
  /** The decision's lines, or the rule's reason. */
  detail: string[];
  /** Whether the entry can be picked: everything but an entry the host's
   * rules do not allow. */
  pickable: boolean;
}

/** A directory entry's verdict in words. */
export function verdictWords(entry: DirectoryVerdict): VerdictWords {
  switch (entry.verdict) {
    case "allowed":
      return { label: "Allowed", tone: "ok", detail: [], pickable: true };
    case "needs_decision":
      return { label: "Needs a decision", tone: "warn", detail: entry.lines, pickable: true };
    case "not_allowed":
      return { label: "Not allowed", tone: "neutral", detail: entry.reason ? [entry.reason] : [], pickable: false };
    default:
      // A verdict this console does not know: shown as it is, and it
      // disables nothing.
      return { label: entry.verdict || "Unknown", tone: "neutral", detail: [...entry.lines, ...(entry.reason ? [entry.reason] : [])], pickable: true };
  }
}

const CONSENT_WORDS: Readonly<Record<string, string>> = {
  allowed: "Allowed by the host's rules",
  decided: "Decided by a person",
  imported: "Decided outside this team and carried in by its host",
};

/** The consent a record's change committed under, in words; a form this
 * console does not know reads as it is. */
export function consentWords(form: string): string {
  return CONSENT_WORDS[form] ?? form;
}

// The surfaces a host asks on. A record says who asked only for an agent
// that was asked for at run time, so the host's configuration is none.
const HOST_SURFACE_WORDS: Readonly<Record<string, string>> = {
  console: "This console",
  rpc: "The host's own code",
};

export type RequesterWords =
  /** An agent: shown by its name, as the roster names it. */
  | { kind: "agent"; identity: string }
  /** A person: `name` as the console names a decision's requester. The
   * person's key is not carried on: it is shown nowhere. */
  | { kind: "person"; name: string }
  | { kind: "host"; words: string };

/** Who asked for an agent, as the record says. `personName` is how the
 * console names a person's key elsewhere (the decision cards' own). */
export function requesterWords(provenance: AgentProvenance, personName: (key: string) => string): RequesterWords {
  if (provenance.requesterKind === "agent") return { kind: "agent", identity: provenance.requester };
  if (provenance.requesterKind === "human") {
    return { kind: "person", name: personName(provenance.requester) };
  }
  return { kind: "host", words: Object.hasOwn(HOST_SURFACE_WORDS, provenance.requester) ? HOST_SURFACE_WORDS[provenance.requester] : `The host (${provenance.requester})` };
}

/** What the confirmation of a change of one agent as a whole says. */
export function agentChangeConfirmWords(change: "archive" | "unarchive" | "remove", name: string): { question: string; consequence: string; confirm: string } {
  if (change === "archive") {
    return {
      question: `Archive ${name}?`,
      consequence: "It stops and keeps its name, its record, its conversation and its memory. Messages to it are refused until you unarchive it.",
      confirm: "Archive",
    };
  }
  if (change === "unarchive") {
    return {
      question: `Unarchive ${name}?`,
      consequence: "It comes back on the conversation it had and takes messages again.",
      confirm: "Unarchive",
    };
  }
  return {
    question: `Remove ${name} for good?`,
    consequence: "Its name can never be used again, and this cannot be undone. Archive is the reversible choice: an archived agent keeps its name, its record, its conversation and its memory, and you can bring it back.",
    confirm: "Remove for good",
  };
}
