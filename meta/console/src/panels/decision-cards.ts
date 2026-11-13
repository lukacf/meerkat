// Durable decisions on the Approvals panel, read only. `meta/decisions/list`
// answers each decision the signed-in caller may see: whole (`decisions`)
// when the caller holds its approver id, or may read every decision, and as
// a status only (`statuses`) when the caller requested it and its kind keeps
// what it holds from the requester. A card shows the rendering of a whole
// decision; a status card never holds any of its content. The console
// decides none of them: a decision is decided where it was presented.

import { humanizeEnum } from "../../../packages/console-components/src/panel-kit";

export interface DecisionPresentationView {
  channelRef: string;
  messageRef: string;
  atMs: number;
}

export interface DecisionCardView {
  decisionId: string;
  kind: string;
  /** The wire phase: open, approved, rejected, expired, withdrawn, executed
   *  or execution_refused (a phase this console does not know shows as it
   *  is). */
  phase: string;
  deadlineAtMs?: number;
  decidedAtMs?: number;
  endedAtMs?: number;
  approverId?: string;
  approverName?: string;
  requester?: string;
  requesterName?: string;
  /** The decision's rendering, when it was read. */
  rendering?: string;
  presentations?: readonly DecisionPresentationView[];
  /** The caller may read the decision's status, not what it holds. */
  statusOnly: boolean;
}

type RawRecord = Record<string, unknown>;

function recordOf(value: unknown): RawRecord | null {
  return value !== null && typeof value === "object" && !Array.isArray(value) ? value as RawRecord : null;
}

function textOf(value: unknown): string | undefined {
  return typeof value === "string" && value.trim() ? value : undefined;
}

function msOf(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? value : undefined;
}

/** The words an approver id reads as: its last segment, humanized
 *  (`team:owners` reads "Owners"). */
export function approverName(approverId: string): string {
  const segment = approverId.slice(approverId.lastIndexOf(":") + 1).trim();
  return humanizeEnum(segment || approverId);
}

/** The id a principal key (`principal:["human","domain","carol"]`) names,
 *  or the key itself when it is not one. */
export function principalName(key: string): string {
  if (!key.startsWith("principal:")) return key;
  try {
    const parts: unknown = JSON.parse(key.slice("principal:".length));
    if (Array.isArray(parts)) {
      const id = parts[parts.length - 1];
      if (typeof id === "string" && id.trim()) return id;
    }
  } catch {
    // A key this console cannot read shows as it is.
  }
  return key;
}

function presentationsOf(value: unknown): DecisionPresentationView[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    const record = recordOf(entry);
    const channelRef = textOf(record?.channel_ref);
    const messageRef = textOf(record?.message_ref);
    const atMs = msOf(record?.at_ms);
    return channelRef && messageRef && atMs !== undefined ? [{ channelRef, messageRef, atMs }] : [];
  });
}

function wholeCard(decision: RawRecord, renderings: ReadonlyMap<string, string>): DecisionCardView | null {
  const decisionId = textOf(decision.decision_id);
  const kind = textOf(decision.kind);
  const phase = textOf(decision.phase);
  if (!decisionId || !kind || !phase) return null;
  const approverId = textOf(decision.approver_id);
  const requester = textOf(decision.requester);
  return {
    decisionId,
    kind,
    phase,
    deadlineAtMs: msOf(decision.deadline_at_ms),
    decidedAtMs: msOf(decision.decided_at_ms),
    endedAtMs: msOf(decision.ended_at_ms),
    approverId,
    approverName: approverId ? approverName(approverId) : undefined,
    requester,
    requesterName: requester ? principalName(requester) : undefined,
    rendering: renderings.get(decisionId),
    presentations: presentationsOf(decision.presentations),
    statusOnly: false,
  };
}

function statusCard(status: RawRecord): DecisionCardView | null {
  const decisionId = textOf(status.decision_id);
  const kind = textOf(status.kind);
  const phase = textOf(status.phase);
  if (!decisionId || !kind || !phase) return null;
  return {
    decisionId,
    kind,
    phase,
    deadlineAtMs: msOf(status.deadline_at_ms),
    decidedAtMs: msOf(status.decided_at_ms),
    endedAtMs: msOf(status.ended_at_ms),
    statusOnly: true,
  };
}

/** The decision ids of a list page whose rendering the caller may read. */
export function wholeDecisionIds(page: unknown): string[] {
  const decisions = recordOf(page)?.decisions;
  if (!Array.isArray(decisions)) return [];
  return decisions.flatMap((entry) => {
    const id = textOf(recordOf(entry)?.decision_id);
    return id ? [id] : [];
  });
}

/** Soonest deadline first; the decision id breaks ties. */
function byDeadline(left: DecisionCardView, right: DecisionCardView): number {
  return (left.deadlineAtMs ?? Number.MAX_SAFE_INTEGER) - (right.deadlineAtMs ?? Number.MAX_SAFE_INTEGER)
    || left.decisionId.localeCompare(right.decisionId);
}

/** The cards of one `meta/decisions/list` page, soonest deadline first;
 *  `renderings` holds the renderings read for its whole decisions. */
export function decisionCardsOf(page: unknown, renderings: ReadonlyMap<string, string>): DecisionCardView[] {
  const record = recordOf(page);
  if (!record) return [];
  const whole = Array.isArray(record.decisions)
    ? record.decisions.flatMap((entry) => {
      const decision = recordOf(entry);
      const card = decision ? wholeCard(decision, renderings) : null;
      return card ? [card] : [];
    })
    : [];
  const statuses = Array.isArray(record.statuses)
    ? record.statuses.flatMap((entry) => {
      const status = recordOf(entry);
      const card = status ? statusCard(status) : null;
      return card ? [card] : [];
    })
    : [];
  return [...whole, ...statuses].sort(byDeadline);
}

/** The params of one `meta/decisions/list` page of open decisions: at most
 *  `limit` rows, after the cursor `after` when the panel follows one. */
export function decisionPageParams(limit: number, after?: string): Record<string, unknown> {
  return after === undefined ? { phase: "open", limit } : { phase: "open", limit, after };
}

/** The cursor of one `meta/decisions/list` page: the decision id to read the
 *  next page after, present only while more may follow. */
export function decisionPageCursor(page: unknown): string | undefined {
  return textOf(recordOf(page)?.next_after);
}

/** `current` and the cards of a further page, one card per decision (the
 *  later read wins), soonest deadline first. */
export function mergeDecisionCards(
  current: readonly DecisionCardView[],
  more: readonly DecisionCardView[],
): DecisionCardView[] {
  const byId = new Map<string, DecisionCardView>();
  for (const card of [...current, ...more]) byId.set(card.decisionId, card);
  return [...byId.values()].sort(byDeadline);
}

/** The rendering of one `meta/decisions/rendering` answer. */
export function renderingOf(answer: unknown): string | undefined {
  const rendering = recordOf(answer)?.rendering;
  return typeof rendering === "string" ? rendering : undefined;
}

/** A phase in words: "Open", "Execution refused". */
export function phaseWords(phase: string): string {
  return humanizeEnum(phase);
}

const IDENTITY_CHANGE_KIND_WORDS: Readonly<Record<string, string>> = {
  "identity.create": "New long-term agent",
  "identity.update": "Change of a long-term agent",
  "identity.archive": "Archive of a long-term agent",
  "identity.unarchive": "Unarchive of a long-term agent",
  "identity.remove": "Removal of a long-term agent",
  "identity.adopt": "Release of an agent from configuration",
};

/** Whether a decision is held for a change of a long-term agent. */
export function isIdentityChangeDecision(kind: string): boolean {
  return kind.startsWith("identity.");
}

/** A decision's kind in words; a kind this console has no words for reads
 *  as its token in words. */
export function decisionKindWords(kind: string): string {
  return IDENTITY_CHANGE_KIND_WORDS[kind] ?? humanizeEnum(kind);
}

/**
 * The withdrawal of this page's own requests from their decision cards.
 * `decisionIds` are the decisions the gateway answered this page's own
 * changes with: the cards the console knows to be the requester's own, from
 * that answer and nothing else. `withdraw` withdraws the request such a
 * decision is held for and answers null, or what the console says when the
 * withdrawal is refused: its own sentence, and for the card's Details the
 * gateway's own where it gave one.
 */
export interface OwnDecisionWithdrawal {
  decisionIds: ReadonlySet<string>;
  withdraw: (decisionId: string) => Promise<WithdrawalRefused | null>;
}

export interface WithdrawalRefused {
  sentence: string;
  detail?: string;
}
