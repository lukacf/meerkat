// Changes of the mob's long-term agents, as the console sends and reads
// them: the `meta/identity/*` family (meerkat-meta
// identity_creation/family.rs and outcome.rs). The console owns no meaning
// here. It writes the request a form describes, reads the typed outcome and
// the typed refusal, and says both in words; who may do what is the
// gateway's answer (the experience read's `identity_changes` section and
// each agent's two affordances), never the console's.

import type { ConsoleAgentAffordances, ConsoleExperience } from "../types";

/** The JSON-RPC code of every refusal of the family. */
export const IDENTITY_REFUSED_CODE = -32058;

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

/** The refusal an error of the console transport carries: the family's one
 * JSON-RPC error. Null for any other failure. */
export function identityRefusalOf(error: unknown): IdentityRefusal | null {
  const rpc = (error as { rpcError?: { code?: unknown; message?: unknown; data?: unknown } } | null)?.rpcError;
  if (rpc?.code !== IDENTITY_REFUSED_CODE) return null;
  return refusalOf(rpc.data, rpc.message);
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

/** The form of an agent as its record reads now. */
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
        lines.push(seat ?? "It starts with its first message.");
      } else {
        title = `${name} is updated`;
      }
  }
  const instructions = instructionOutcomeWords(agent?.instructions);
  if (instructions) lines.push(instructions);
  return { tone: "ok", title, lines };
}

/** The sentence of a refusal: the gateway's own. A refusal that brings
 * none gets the console's plain one; a kind token is never the sentence. */
export function refusalSentence(refusal: IdentityRefusal): string {
  return refusal.detail || "The change was refused.";
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

const HOST_SURFACE_WORDS: Readonly<Record<string, string>> = {
  console: "This console",
  rpc: "The host's own code",
  configuration: "The host's configuration",
};

export type RequesterWords =
  /** An agent: shown by its name, as the roster names it. */
  | { kind: "agent"; identity: string }
  /** A person: `name` as the console names a decision's requester, `key`
   * the tooltip. */
  | { kind: "person"; name: string; key: string }
  | { kind: "host"; words: string };

/** Who asked for an agent, as the record says. `personName` is how the
 * console names a person's key elsewhere (the decision cards' own). */
export function requesterWords(provenance: AgentProvenance, personName: (key: string) => string): RequesterWords {
  if (provenance.requesterKind === "agent") return { kind: "agent", identity: provenance.requester };
  if (provenance.requesterKind === "human") {
    return { kind: "person", name: personName(provenance.requester), key: provenance.requester };
  }
  return { kind: "host", words: HOST_SURFACE_WORDS[provenance.requester] ?? `The host (${provenance.requester})` };
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
