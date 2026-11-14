import { CONSOLE_RPC_METHODS } from "./contract";
import type { ApprovalResourceEnvironment } from "./pending-approvals";

/** The most UTF-8 bytes a free-text answer may hold (the gateway's limit). */
export const FREE_TEXT_MAX_BYTES = 280;
export interface QuestionChoice {
  id: string;
  label: string;
}
export interface Question {
  id: string;
  prompt: string;
  choices: readonly QuestionChoice[];
  /** Whether a person may write their own answer instead of a choice. */
  allowFreeText: boolean;
  /** The choice the asking agent recommends. Shown, never preselected. */
  recommended?: string;
  /** How many choices past the reader's limit are not shown. */
  omittedChoices?: number;
}
/** One open set of multiple-choice questions an agent asked a person
 * (`meta/gating/questions`). */
export interface QuestionSet {
  pendingId: string;
  /** The asking agent. */
  origin: { identity: string };
  title?: string;
  questions: readonly Question[];
  createdAtMs?: number;
  deadlineAtMs?: number;
  /** The digest of what was presented; a decision sends it back. */
  subjectDigest: string;
  /** How many questions past the reader's limit are not shown. */
  omittedQuestions?: number;
  /** A title, prompt or label was longer than the reader's limit and is
   * shown cut. */
  shortened?: true;
}
export type QuestionAnswer = { kind: "choice"; choiceId: string } | { kind: "free_text"; text: string };
/** A person's answers by question id. A question without an entry is left
 * out and reaches the agent as unanswered. */
export type QuestionSetAnswers = Readonly<Record<string, QuestionAnswer>>;
export type QuestionSetWireAnswer = {
  question_id: string;
  answer: { kind: "choice"; choice_id: string } | { kind: "free_text"; text: string };
};
export interface QuestionSetAnswerParams {
  pending_id: string;
  decision: "approve";
  presented_digest: string;
  answers: QuestionSetWireAnswer[];
}
export interface QuestionSetDismissParams {
  pending_id: string;
  decision: "reject";
  presented_digest: string;
}
/** What the gateway said of a decide it did not take. Nothing was sent to
 * the agent in any of them.
 * - `not_open`, `expired`: the decision's typed refusal; the set is no
 *   longer answerable and its card goes.
 * - `subject_changed`, `not_eligible`: the decision's typed refusal; the
 *   set stays listed.
 * - `not_taken`: any other typed refusal of the decision.
 * - `not_accepted`: invalid params without the typed refusal, for a set
 *   the gateway still lists.
 * - `approver_unauthenticated`, `access_denied`: the person (not signed in
 *   as a person; lost the permission to decide).
 * - `caller_changed`, `caller_not_current`: the request's caller.
 * - `not_sent`: the request never reached the gateway's decide (the
 *   console did not know whom it sends for and sent nothing, a sign-in
 *   page answered instead of the gateway, or the login had ended). */
export type QuestionSetRefusalKind =
  | "not_open" | "expired" | "subject_changed" | "not_eligible" | "not_taken" | "not_accepted"
  | "approver_unauthenticated" | "access_denied" | "caller_changed" | "caller_not_current" | "not_sent";
/** A decision the gateway refused: nothing was sent to the agent. */
export interface QuestionSetRefusal {
  kind: QuestionSetRefusalKind;
  /** The reason in words for the person answering, with what to do. */
  words: string;
  /** What the gateway said, where the words do not say it all: its own
   * sentence for a reply it did not accept, and only the kind of a refusal
   * this console does not know by name (the gateway's message for one
   * carries the decision's id). */
  detail?: string;
}
export interface QuestionSetDecisionState {
  /** Unavailable means a typed capability refusal before dispatch; refused a
   * typed refusal by the gateway after dispatch (nothing was decided); failed
   * retains outcome uncertainty. */
  phase: "submitting" | "answered" | "dismissed" | "failed" | "refused" | "unavailable";
  action: "answer" | "dismiss";
  /** What was sent, for the card's summary. */
  answers?: QuestionSetAnswers;
  decidedAtMs?: number;
  error?: string;
  refusal?: QuestionSetRefusal;
}
/** How a set this console still shows ended: answered or dismissed here,
 * with the gateway's own result. A set that left the list any other way is
 * not kept. */
export type ClosedQuestionSetOutcome = "answered" | "dismissed";
export interface ClosedQuestionSet {
  set: QuestionSet;
  outcome: ClosedQuestionSetOutcome;
  closedAtMs: number;
}
/** Why a card left the screen: the read no longer lists its set (`gone`),
 * it did so after a send this console could not confirm
 * (`sent_unconfirmed`), or the gateway refused this console's decide
 * because the set is no longer open (`not_open`) or expired (`expired`). */
export type QuestionSetNoticeKind = "gone" | "sent_unconfirmed" | "not_open" | "expired";
/** One neutral line for a card that left. It holds nothing of the set: the
 * card's own place on this page (`slot`), why it left, and when. */
export interface QuestionSetNotice {
  slot: number;
  kind: QuestionSetNoticeKind;
  atMs: number;
}
export interface QuestionSetSnapshot {
  scopeKey: string;
  /** Unsupported comes from a gateway without question sets (shown as
   * nothing); unavailable is a transient read failure. */
  status: "loading" | "ready" | "stale" | "unavailable" | "forbidden" | "unsupported";
  /** The open sets, as the gateway lists them: soonest deadline first. */
  sets: readonly QuestionSet[];
  /** Sets this console answered or dismissed, newest first. */
  closed: readonly ClosedQuestionSet[];
  decisions: Readonly<Record<string, QuestionSetDecisionState>>;
  /** The lines of the cards that left, newest first; each goes a minute
   * after its card. */
  notices: readonly QuestionSetNotice[];
  /** The place of each listed or closed set's card on this page: a number
   * this console counts, by the set's `pendingId`. */
  slots: Readonly<Record<string, number>>;
  readOnly: boolean;
  /** Why the sets are read-only, when the console knows it: `sign_in` when
   * the caller is no signed-in person (only a person answers), and
   * `no_permission` after the gateway said the person may no longer
   * decide. A reason the gateway gave on a decide holds until a later read
   * of the list succeeds. */
  readOnlyReason?: "sign_in" | "no_permission";
  updatedAtMs?: number;
  error?: string;
}
/** One card: an open set, or one this console answered or dismissed. */
export interface QuestionSetEntry {
  set: QuestionSet;
  decision?: QuestionSetDecisionState;
  closed?: ClosedQuestionSetOutcome;
  /** The card's place on this page (see the snapshot's `slots`). */
  slot?: number;
}
export interface QuestionSetResource {
  getSnapshot(): QuestionSetSnapshot;
  subscribe(listener: () => void): () => void;
  refresh(): Promise<void>;
  answer(pendingId: string, answers: QuestionSetAnswers): Promise<void>;
  dismiss(pendingId: string): Promise<void>;
  dispose(): void;
}

const POLL_MS = 15_000;
const STALE_MS = 2 * POLL_MS;
/** How many answered and dismissed sets the snapshot keeps. */
const CLOSED_LIMIT = 20;
/** How many lines of cards that left the snapshot keeps, and for how long. */
const NOTICE_LIMIT = 5;
const NOTICE_KEEP_MS = 60_000;
/** The most this console reads of one set: the gateway's own limits (the
 * text lengths in characters, which never cut what fits the gateway's
 * bytes). What a server sends beyond them is left out, and the card says
 * so. */
export const QUESTION_SET_READ_LIMITS = { questions: 10, choices: 8, title: 200, prompt: 1024, label: 256 } as const;
const text = (value: unknown) => typeof value === "string" && value.trim() ? value : undefined;
const millis = (value: unknown) => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 8_640_000_000_000_000 ? value : undefined;
const record = (value: unknown) => value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
/** A table by an id a server supplies (a set's `pending_id`, a question's
 * `id`). It has no prototype: an id such as `__proto__` or `constructor` is
 * an entry like any other, and an id without an entry reads as undefined. */
function table<T>(entries: Iterable<readonly [string, T]> = []): Record<string, T> {
  const rows: Record<string, T> = Object.create(null);
  for (const [id, value] of entries) rows[id] = value;
  return rows;
}
/** The entry a table has for the id itself, never one it inherits: a
 * snapshot a host or a test built may be a plain object. */
const own = <T>(rows: Readonly<Record<string, T>>, id: string): T | undefined => (
  Object.prototype.hasOwnProperty.call(rows, id) ? rows[id] : undefined
);
/** White space, controls and format characters (zero-width and direction
 * marks, the byte order mark, the soft hyphen). */
const INVISIBLE = /[\s\p{Cc}\p{Cf}\p{Z}]/u;
/** Other characters that show nothing: the grapheme joiner, the Hangul
 * fillers, two Khmer vowels and the blank Braille pattern; and the
 * variation selectors. */
const BLANK = new Set([0x34f, 0x115f, 0x1160, 0x17b4, 0x17b5, 0x2800, 0x3164, 0xffa0]);
const selector = (point: number) => (point >= 0x180b && point <= 0x180f) || (point >= 0xfe00 && point <= 0xfe0f) || (point >= 0xe0100 && point <= 0xe01ef);
/** Whether a person would see anything of the string. */
function shows(value: string): boolean {
  for (const character of value) {
    const point = character.codePointAt(0) ?? 0;
    if (!INVISIBLE.test(character) && !BLANK.has(point) && !selector(point)) return true;
  }
  return false;
}
/** A text a person reads (a title, prompt or label): undefined when it is
 * no string or shows nothing, and cut to `limit` whole characters. */
function words(value: unknown, limit: number): { text: string; cut: boolean } | undefined {
  if (typeof value !== "string" || !shows(value)) return undefined;
  if (value.length <= limit) return { text: value, cut: false };
  const characters = Array.from(value.slice(0, 2 * limit + 1));
  if (characters.length <= limit) return { text: value, cut: false };
  const kept = characters.slice(0, limit).join("");
  return shows(kept) ? { text: kept, cut: true } : undefined;
}

function normalizeQuestion(value: unknown): { question: Question; cut: boolean } | null {
  const raw = record(value);
  const id = text(raw?.id), prompt = words(raw?.prompt, QUESTION_SET_READ_LIMITS.prompt);
  if (!raw || !id || !prompt || !Array.isArray(raw.choices) || raw.choices.length === 0) return null;
  const choices: QuestionChoice[] = [];
  let cut = prompt.cut;
  // The choices past the limit are not read.
  for (const entry of raw.choices.slice(0, QUESTION_SET_READ_LIMITS.choices)) {
    const choice = record(entry);
    const choiceId = text(choice?.id), label = words(choice?.label, QUESTION_SET_READ_LIMITS.label);
    if (!choiceId || !label || choices.some((known) => known.id === choiceId)) return null;
    choices.push({ id: choiceId, label: label.text });
    cut = cut || label.cut;
  }
  const recommended = text(raw.recommended);
  const omittedChoices = raw.choices.length - choices.length;
  return {
    question: {
      id, prompt: prompt.text, choices, allowFreeText: raw.allow_free_text === true,
      // A recommendation that names no choice shown is not shown.
      ...(recommended && choices.some((choice) => choice.id === recommended) ? { recommended } : {}),
      ...(omittedChoices > 0 ? { omittedChoices } : {}),
    },
    cut,
  };
}

/** A tolerant reader: unknown fields are ignored, and a set this console
 * cannot present whole (no id, asking agent, digest or readable questions)
 * is dropped. A title, prompt or label of only invisible characters is
 * empty. It reads at most `QUESTION_SET_READ_LIMITS` and says what it left
 * out. Never throws. */
export function normalizeQuestionSet(value: unknown): QuestionSet | null {
  const raw = record(value);
  const pendingId = text(raw?.pending_id), identity = text(record(raw?.origin)?.identity), subjectDigest = text(raw?.subject_digest);
  if (!raw || !pendingId || !identity || !subjectDigest || !Array.isArray(raw.questions) || raw.questions.length === 0) return null;
  const questions: Question[] = [];
  const title = words(raw.title, QUESTION_SET_READ_LIMITS.title);
  let cut = title?.cut === true;
  // The questions past the limit are not read.
  for (const entry of raw.questions.slice(0, QUESTION_SET_READ_LIMITS.questions)) {
    const read = normalizeQuestion(entry);
    if (!read || questions.some((known) => known.id === read.question.id)) return null;
    questions.push(read.question);
    cut = cut || read.cut;
  }
  const createdAtMs = millis(raw.created_at_ms), deadlineAtMs = millis(raw.deadline_at_ms);
  const omittedQuestions = raw.questions.length - questions.length;
  return {
    pendingId, origin: { identity }, ...(title ? { title: title.text } : {}), questions,
    ...(createdAtMs !== undefined ? { createdAtMs } : {}), ...(deadlineAtMs !== undefined ? { deadlineAtMs } : {}),
    subjectDigest,
    ...(omittedQuestions > 0 ? { omittedQuestions } : {}), ...(cut ? { shortened: true as const } : {}),
  };
}

export function utf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

/** Whether this console shows the set whole: no question or choice was
 * left out by the reader and no text was cut. A reply sends back the digest
 * of the whole set, so only a set shown whole takes one from here. */
export function questionSetShownWhole(set: QuestionSet): boolean {
  return !set.omittedQuestions && !set.shortened && set.questions.every((question) => !question.omittedChoices);
}

/** The answers of the set's own questions, in the set's order, as the wire
 * carries them. Free text goes without the whitespace at its ends. */
function wireAnswers(set: QuestionSet, answers: QuestionSetAnswers): QuestionSetWireAnswer[] {
  return set.questions.flatMap((question): QuestionSetWireAnswer[] => {
    const answer = own(answers, question.id);
    if (!answer) return [];
    return [{
      question_id: question.id,
      answer: answer.kind === "choice" ? { kind: "choice", choice_id: answer.choiceId } : { kind: "free_text", text: answer.text.trim() },
    }];
  });
}

/** The answers the set offers now: for a question it has, a choice of that
 * question, or the person's own words where the question allows them. A set
 * that changed may no longer offer what a draft holds. */
export function questionSetAnswersOffered(set: QuestionSet, answers: QuestionSetAnswers): QuestionSetAnswers {
  const offered = table<QuestionAnswer>();
  for (const question of set.questions) {
    const answer = own(answers, question.id);
    if (!answer) continue;
    if (answer.kind === "choice" ? question.choices.some((choice) => choice.id === answer.choiceId) : question.allowFreeText) offered[question.id] = answer;
  }
  return offered;
}

/** Whether the answers can be sent: at least one question is answered, each
 * choice is one of its question's, and free text is given only where the
 * agent allowed it, with 1 to `FREE_TEXT_MAX_BYTES` bytes. */
export function questionSetAnswersSendable(set: QuestionSet, answers: QuestionSetAnswers): boolean {
  const sent = wireAnswers(set, answers);
  return sent.length > 0 && sent.every(({ question_id, answer }) => {
    const question = set.questions.find((candidate) => candidate.id === question_id)!;
    if (answer.kind === "choice") return question.choices.some((choice) => choice.id === answer.choice_id);
    const bytes = utf8ByteLength(answer.text);
    return question.allowFreeText && bytes >= 1 && bytes <= FREE_TEXT_MAX_BYTES;
  });
}

/** The `meta/gating/decide` request that answers the set. It names no
 * approver: the gateway decides as the signed-in person. */
export function questionSetAnswerParams(set: QuestionSet, answers: QuestionSetAnswers): QuestionSetAnswerParams {
  return { pending_id: set.pendingId, decision: "approve", presented_digest: set.subjectDigest, answers: wireAnswers(set, answers) };
}

/** The `meta/gating/decide` request that dismisses the set. */
export function questionSetDismissParams(set: QuestionSet): QuestionSetDismissParams {
  return { pending_id: set.pendingId, decision: "reject", presented_digest: set.subjectDigest };
}

/** The gateway's refusals of a decide, in words for the person answering:
 * what happened, that nothing was sent, and what to do. They name nothing
 * of the set. `not_sent` has four causes, and in one of them (the login had
 * ended) the request left the browser and was refused at the gateway's
 * door: its words claim only what holds for all four, that the reply was
 * not accepted and that nothing was changed. */
export const QUESTION_SET_REFUSAL_WORDS: Readonly<Record<QuestionSetRefusalKind, string>> = {
  not_open: "This question was already answered or closed, so nothing was sent. Nothing more is needed from you.",
  expired: "This question timed out before the gateway took your reply, so nothing was sent. If the answer still matters, tell the agent in its chat.",
  subject_changed: "These questions changed after you opened them, so nothing was sent. Read them again, then answer.",
  not_eligible: "You are not one of the people who can answer this question, so nothing was sent. Ask an administrator if you should be.",
  not_taken: "The gateway did not take this reply, so nothing was sent. Try again; if it happens again, tell an administrator.",
  not_accepted: "The gateway did not accept this reply, so nothing was sent. Check it, then send again.",
  approver_unauthenticated: "Only a signed-in person can answer, so nothing was sent. Sign in as yourself, then send again.",
  access_denied: "You no longer have permission to answer questions here, so nothing was sent. Ask an administrator for the permission to decide, then send again.",
  caller_changed: "This browser is now signed in as someone else, so nothing was sent or changed. Answer again as the person signed in now.",
  caller_not_current: "Your sign-in ended before the reply was sent, so nothing was changed. Sign in again, then send again.",
  not_sent: "This reply was not accepted. Nothing was changed. Sign in again if you are asked to, then send again.",
};
/** The typed refusals of the decision itself that this console reads by
 * name; every other one reads as `not_taken`. */
const DECISION_REFUSALS: ReadonlySet<string> = new Set(["not_open", "expired", "subject_changed", "not_eligible"]);
/** A refusal kind as the gateway writes one: a short word. Anything else
 * is not shown. */
const KIND_WORD = /^[a-z][a-z_]{0,63}$/;
type RpcFailure = { httpStatus?: unknown; transportFailure?: unknown; rpcError?: { code?: unknown; message?: unknown; data?: unknown }; responseRpcError?: { code?: unknown; message?: unknown; data?: unknown } };
function rpcFailure(error: unknown): { code?: unknown; kind: string; message?: string; data?: Record<string, unknown> } {
  const failure = error as RpcFailure | null;
  const rpc = failure?.rpcError ?? failure?.responseRpcError;
  const data = record(rpc?.data);
  const kind = typeof data?.kind === "string" ? data.kind : typeof data?.error === "string" ? data.error : "";
  return { code: rpc?.code, kind, message: text(rpc?.message), data };
}
const isDenied = (error: unknown) => {
  const { code, kind } = rpcFailure(error);
  return (error as RpcFailure | null)?.httpStatus === 403 || code === -32030 || kind === "access_denied";
};
/** A refusal the gateway states (nothing was decided), or null when the
 * outcome is uncertain. A decide the set itself refuses carries
 * `data.kind = "decision_refused"` and the refusal as an object tagged by
 * `kind` (`data.decision_refusal.kind`). */
export function questionSetRefusal(error: unknown): QuestionSetRefusal | null {
  const { code, kind, message, data } = rpcFailure(error);
  const refusal = (name: QuestionSetRefusalKind, detail?: string): QuestionSetRefusal => (
    { kind: name, words: QUESTION_SET_REFUSAL_WORDS[name], ...(detail ? { detail } : {}) }
  );
  if (kind === "decision_refused") {
    const token = record(data?.decision_refusal)?.kind;
    if (typeof token === "string" && DECISION_REFUSALS.has(token)) return refusal(token as QuestionSetRefusalKind);
    // A refusal this console does not know by name: its kind is all that
    // is kept. The gateway's message for it names the decision by its id.
    return refusal("not_taken", typeof token === "string" && KIND_WORD.test(token) ? `The gateway's refusal: ${token}` : undefined);
  }
  // The request never reached the gateway's decide: the console sent
  // nothing (it did not know whom it sends for), a sign-in page answered in
  // the gateway's place, or the login had ended.
  const failure = error as RpcFailure | null;
  if (failure?.transportFailure === "caller_unknown" || failure?.transportFailure === "sign_in_redirect"
    || failure?.httpStatus === 401 || kind === "unauthenticated") return refusal("not_sent");
  if (kind === "caller_changed" || kind === "caller_not_current") return refusal(kind);
  if (kind === "approver_unauthenticated" || code === -32057) return refusal("approver_unauthenticated");
  // A question set's decide is denied for one reason: the person lost the
  // permission to decide.
  if (isDenied(error)) return refusal("access_denied");
  // Invalid params that say no more: an answer the gateway does not accept,
  // or an id that is not pending. The next read tells them apart.
  if (code === -32602) return refusal("not_accepted", message);
  return null;
}
function unavailableCapability(error: unknown): { method: string; availableMethods: readonly string[] } | null {
  const failure = error as { kind?: unknown; method?: unknown; availableMethods?: unknown } | null;
  return failure?.kind === "console-capability-unavailable" && typeof failure.method === "string"
    && Array.isArray(failure.availableMethods) && failure.availableMethods.every((method) => typeof method === "string")
    ? { method: failure.method, availableMethods: failure.availableMethods } : null;
}
/** A gateway older than question sets: its capabilities do not list the
 * read, or it answers method-not-found. */
const isUnsupported = (error: unknown) => unavailableCapability(error)?.method === CONSOLE_RPC_METHODS.gatingQuestions
  || rpcFailure(error).code === -32601;
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

function browserEnvironment(): ApprovalResourceEnvironment {
  return {
    now: Date.now,
    visible: () => typeof document === "undefined" || document.visibilityState !== "hidden",
    setTimeout: (callback, ms) => globalThis.setTimeout(callback, ms),
    clearTimeout: (timer) => globalThis.clearTimeout(timer as ReturnType<typeof setTimeout>),
    onVisibilityChange(callback) {
      if (typeof document === "undefined") return () => {};
      document.addEventListener("visibilitychange", callback);
      return () => document.removeEventListener("visibilitychange", callback);
    },
  };
}

/** The cards of a snapshot: the open sets as listed, then the ones this
 * console answered or dismissed, newest first. With `identity`, only the
 * sets that agent asked. */
export function questionSetEntries(snapshot: QuestionSetSnapshot, identity?: string): QuestionSetEntry[] {
  const mine = (set: QuestionSet) => identity === undefined || set.origin.identity === identity;
  const slot = (set: QuestionSet) => {
    const place = own(snapshot.slots, set.pendingId);
    return place === undefined ? {} : { slot: place };
  };
  return [
    ...snapshot.sets.filter(mine).map((set) => ({ set, decision: own(snapshot.decisions, set.pendingId), ...slot(set) })),
    ...snapshot.closed.filter((entry) => mine(entry.set)).map((entry) => ({ set: entry.set, decision: own(snapshot.decisions, entry.set.pendingId), closed: entry.outcome, ...slot(entry.set) })),
  ];
}

/** How many open sets a count may hold: those of a list that was read. A
 * list that is still being read, may be out of date or could not be read
 * counts none (as approvals that are not read count none), and what shows
 * the count says so in words. One rule for "Needs you", the Approvals badge
 * and the inbox's Pending tab, so their numbers agree. A host that does not
 * know the state of the read (`status` undefined) counts what it was given. */
export function countedQuestionSets(status: QuestionSetSnapshot["status"] | undefined, open: number): number {
  return status === undefined || status === "ready" ? open : 0;
}

/** One active scoped owner feeds the inbox and every conversation. It polls
 * like the approvals resource: every 15 s while the page is visible, and on
 * becoming visible. */
export function createQuestionSetResource(input: {
  scopeKey: string;
  load(signal: AbortSignal): Promise<unknown>;
  /** Sends one `meta/gating/decide` request. */
  decide(params: QuestionSetAnswerParams | QuestionSetDismissParams, signal: AbortSignal): Promise<unknown>;
  readOnly?: boolean;
  /** Why `readOnly` holds, for the cards' words (see the snapshot's field). */
  readOnlyReason?: "sign_in";
  environment?: ApprovalResourceEnvironment;
}): QuestionSetResource {
  const env = input.environment || browserEnvironment();
  let snapshot: QuestionSetSnapshot = {
    scopeKey: input.scopeKey, status: "loading", sets: [], closed: [], decisions: table(), notices: [], slots: table(), readOnly: input.readOnly === true,
    ...(input.readOnly === true && input.readOnlyReason ? { readOnlyReason: input.readOnlyReason } : {}),
  };
  const listeners = new Set<() => void>();
  const lifetime = new AbortController();
  let disposed = false;
  let denied = false;
  let timer: unknown;
  let activeRead: Promise<void> | undefined;
  let invalidated = false;
  let decisionGeneration = 0;
  /** The decisions on their way. No read is applied meanwhile: each
   * decision is followed by a read of its own. */
  let deciding = 0;
  const decisionJobs = new Map<string, Promise<void>>();
  // The place of each card on this page: counted here, never read from a set.
  const places = new Map<string, number>();
  let lastPlace = 0;
  /** The gateway's capabilities stopped listing the decide. */
  let undecidable = false;
  /** What the gateway said of the person on a decide: every card is
   * read-only, through the decide's own read (`reads`) and until the
   * successful read after it. The gateway's read does not say whether the
   * person may decide again, so the cards then offer Send again. */
  let hold: { reason: "sign_in" | "no_permission"; reads: number } | undefined;
  /** The refusals that wait for a read: the gateway answers an id that is
   * not pending the same way as an answer it does not accept, and only the
   * list says which it was. By the set's id, what the card says if the set
   * is still listed. */
  const unsettled = new Map<string, QuestionSetDecisionState>();
  const access = (): Pick<QuestionSetSnapshot, "readOnly" | "readOnlyReason"> => {
    if (input.readOnly === true) return { readOnly: true, readOnlyReason: input.readOnlyReason };
    if (undecidable) return { readOnly: true, readOnlyReason: undefined };
    return hold ? { readOnly: true, readOnlyReason: hold.reason } : { readOnly: false, readOnlyReason: undefined };
  };
  const publish = (patch: Partial<QuestionSetSnapshot>) => {
    if (disposed) return;
    snapshot = { ...snapshot, ...patch };
    for (const listener of listeners) listener();
  };
  const stale = () => {
    if (snapshot.status === "ready" && snapshot.updatedAtMs !== undefined && env.now() - snapshot.updatedAtMs >= STALE_MS) publish({ status: "stale" });
  };
  /** Ends the resource for this scope: nothing is listed and nothing more is read. */
  const end = (status: "forbidden" | "unsupported", error?: unknown) => {
    denied = true;
    places.clear();
    unsettled.clear();
    hold = undefined;
    publish({ sets: [], closed: [], decisions: table(), notices: [], slots: table(), status, readOnly: true, readOnlyReason: undefined, error: error === undefined ? undefined : errorText(error) });
    lifetime.abort();
  };
  const close = (sets: readonly QuestionSet[], outcome: ClosedQuestionSetOutcome): ClosedQuestionSet[] => [
    ...sets.map((set) => ({ set, outcome, closedAtMs: env.now() })),
    ...snapshot.closed.filter((old) => !sets.some((set) => set.pendingId === old.set.pendingId)),
  ].slice(0, CLOSED_LIMIT);
  /** The decisions and the places of the listed and the closed sets, and of
   * no other: what a set had here goes with it. */
  const keptFor = (sets: readonly QuestionSet[], closed: readonly ClosedQuestionSet[], decisions: QuestionSetSnapshot["decisions"]) => {
    const kept = new Set([...sets.map((set) => set.pendingId), ...closed.map((entry) => entry.set.pendingId)]);
    for (const id of Array.from(places.keys())) if (!kept.has(id)) places.delete(id);
    return {
      decisions: table(Object.entries(decisions).filter(([id]) => kept.has(id))),
      slots: table(places),
    };
  };
  const schedule = () => {
    if (timer !== undefined) env.clearTimeout(timer);
    timer = undefined;
    if (disposed || denied || !env.visible()) return;
    timer = env.setTimeout(() => {
      timer = undefined;
      stale();
      // A slow read owns the sole slot. Poll ticks do not enqueue more reads.
      if (!activeRead) void refresh(false);
      schedule();
    }, POLL_MS);
  };
  const refresh = (explicit: boolean): Promise<void> => {
    if (disposed || denied) return Promise.resolve();
    if (activeRead) {
      if (explicit) invalidated = true;
      return activeRead;
    }
    const generation = decisionGeneration;
    activeRead = (async () => {
      try {
        const value = await Promise.resolve().then(() => input.load(lifetime.signal));
        if (disposed || denied) return;
        if (generation !== decisionGeneration) { invalidated = true; return; }
        // A decision is on its way: its answer says what became of its set,
        // and the read that follows it lists the rest.
        if (deciding > 0) return;
        const raw = Array.isArray(value) ? value : record(value)?.question_sets;
        if (!Array.isArray(raw)) throw new Error("Question set response is unavailable");
        const sets: QuestionSet[] = [];
        const ids = new Set<string>();
        // A set this console cannot present is left out; the others stay.
        for (const row of raw) {
          const set = normalizeQuestionSet(row);
          if (set && !ids.has(set.pendingId)) { ids.add(set.pendingId); sets.push(set); }
        }
        const now = env.now();
        // A set the gateway no longer lists leaves, whatever this clock says
        // about its deadline, and nothing of it is kept: its card's place gets
        // one line. Only a set this console answered or dismissed, with the
        // gateway's own result, stays as a closed card.
        const left = snapshot.sets.filter((old) => !ids.has(old.pendingId));
        const lines: QuestionSetNotice[] = [];
        for (const old of left) {
          const slot = places.get(old.pendingId);
          places.delete(old.pendingId);
          if (slot === undefined) continue;
          // After a send this console could not confirm, the line says so.
          lines.push({ slot, kind: own(snapshot.decisions, old.pendingId)?.phase === "failed" ? "sent_unconfirmed" : "gone", atMs: now });
        }
        for (const set of sets) if (!places.has(set.pendingId)) places.set(set.pendingId, ++lastPlace);
        const closed = snapshot.closed.filter((old) => !ids.has(old.set.pendingId));
        // A reply the gateway did not accept, without saying whether its set
        // is still there: still listed, the card says so; no longer listed,
        // the set went with the others above.
        let decisions = snapshot.decisions;
        for (const [id, refused] of unsettled) if (ids.has(id)) decisions = { ...decisions, [id]: refused };
        unsettled.clear();
        // What the gateway said of the person holds through the decide's own
        // read; the successful read after it lets the cards offer Send again.
        if (hold) { if (hold.reads > 0) hold.reads -= 1; else hold = undefined; }
        publish({
          sets, closed, ...keptFor(sets, closed, decisions), ...access(),
          notices: [...lines, ...snapshot.notices.filter((line) => now - line.atMs < NOTICE_KEEP_MS)].slice(0, NOTICE_LIMIT),
          status: "ready", updatedAtMs: now, error: undefined,
        });
      } catch (error) {
        if (disposed || denied) return;
        if (isUnsupported(error)) end("unsupported");
        else if (isDenied(error)) end("forbidden", error);
        else if (generation === decisionGeneration) {
          publish({ status: snapshot.updatedAtMs === undefined ? "unavailable" : "stale", error: errorText(error) });
        }
      } finally {
        activeRead = undefined;
        // A read a decision overtook is read again before the caller goes on.
        if (invalidated && !disposed && !denied) { invalidated = false; await refresh(false); }
      }
    })();
    return activeRead;
  };
  const setDecision = (id: string, value: QuestionSetDecisionState) => {
    // Only the listed and the closed sets keep a decision.
    const kept = new Set([...snapshot.sets.map((set) => set.pendingId), ...snapshot.closed.map((entry) => entry.set.pendingId)]);
    publish({ decisions: table(Object.entries({ ...snapshot.decisions, [id]: value }).filter(([key]) => kept.has(key))) });
  };
  /** Takes a set the gateway said is no longer answerable off the screen,
   * with everything of it, and leaves that refusal as its card's one line. */
  const remove = (pendingId: string, kind: QuestionSetNoticeKind) => {
    const slot = places.get(pendingId);
    const open = snapshot.sets.filter((candidate) => candidate.pendingId !== pendingId);
    if (open.length === snapshot.sets.length) return;
    publish({
      sets: open, ...keptFor(open, snapshot.closed, snapshot.decisions),
      notices: slot === undefined ? snapshot.notices : [{ slot, kind, atMs: env.now() }, ...snapshot.notices].slice(0, NOTICE_LIMIT),
    });
  };
  const decide = (pendingId: string, action: "answer" | "dismiss", answers?: QuestionSetAnswers): Promise<void> => {
    if (disposed) return Promise.resolve();
    const existing = decisionJobs.get(pendingId);
    if (existing) return existing;
    const set = snapshot.sets.find((candidate) => candidate.pendingId === pendingId);
    if (snapshot.readOnly || denied || snapshot.status !== "ready" || !set || unsettled.has(pendingId)
      // A reply attests the whole set by its digest: none is sent for a set
      // this console shows only in part.
      || !questionSetShownWhole(set)
      || (action === "answer" && (!answers || !questionSetAnswersSendable(set, answers)))) {
      return Promise.resolve();
    }
    const previous = own(snapshot.decisions, pendingId);
    const sent = action === "answer" ? { answers } : {};
    ++decisionGeneration;
    ++deciding;
    setDecision(pendingId, { phase: "submitting", action, ...sent });
    const job = (async () => {
      try {
        const params = action === "answer" ? questionSetAnswerParams(set, answers!) : questionSetDismissParams(set);
        const result = record(await Promise.resolve().then(() => input.decide(params, lifetime.signal)));
        if (disposed || denied) return;
        const outcome = action === "answer" ? "answered" : "dismissed";
        if (!result || result.pending_id !== pendingId || result.kind !== "question_set" || result.outcome !== outcome) {
          throw new Error("The outcome is unconfirmed; refreshing the question sets");
        }
        const decidedAtMs = millis(result.decided_at_ms);
        // The response is authoritative for this set: it is no longer open.
        const open = snapshot.sets.filter((candidate) => candidate.pendingId !== pendingId);
        const closed = close([set], outcome);
        publish({
          sets: open, closed,
          ...keptFor(open, closed, { ...snapshot.decisions, [pendingId]: { phase: outcome, action, ...sent, ...(decidedAtMs !== undefined ? { decidedAtMs } : {}) } }),
        });
      } catch (error) {
        if (disposed || denied) return;
        const capability = unavailableCapability(error);
        const refusal = questionSetRefusal(error);
        if (capability?.method === CONSOLE_RPC_METHODS.gatingDecide) {
          undecidable = true;
          publish(access());
          // A refused retry says nothing about an earlier dispatched attempt.
          setDecision(pendingId, previous?.phase === "failed" ? previous : {
            phase: "unavailable", action, error: "Answers are unavailable with current access",
          });
        } else if (!refusal) {
          setDecision(pendingId, { phase: "failed", action, ...sent, error: errorText(error) });
        } else if (refusal.kind === "not_open" || refusal.kind === "expired") {
          // The gateway says the set is no longer answerable: its card goes.
          remove(pendingId, refusal.kind);
        } else if (refusal.kind === "not_accepted") {
          // The set reads as sending until a read has said whether it is
          // still listed; it is never shown as refused before that.
          unsettled.set(pendingId, { phase: "refused", action, ...sent, refusal, error: refusal.words });
        } else if (refusal.kind === "not_sent" && previous?.phase === "failed") {
          // Like the capability refusal above: a retry that never reached
          // the gateway's decide says nothing about the earlier reply, which
          // may have arrived. The card stays "Not confirmed", and the line
          // of a set that then leaves still says so.
          setDecision(pendingId, previous);
        } else {
          // What the gateway said of the person holds for every card: each
          // would be refused in turn.
          if (refusal.kind === "access_denied") hold = { reason: "no_permission", reads: 1 };
          else if (refusal.kind === "approver_unauthenticated") hold = { reason: "sign_in", reads: 1 };
          if (hold) publish(access());
          setDecision(pendingId, { phase: "refused", action, ...sent, refusal, error: refusal.words });
        }
      } finally {
        ++decisionGeneration;
        --deciding;
        if (!disposed && !denied) await refresh(true);
        // The read that follows the decision settles a refusal that waited
        // for it. When that read failed, the last list stands: the reply was
        // not accepted, and the next read that succeeds says the rest. (While
        // another decision is on its way, its own read settles this one.)
        const waiting = unsettled.get(pendingId);
        if (waiting && deciding === 0 && !disposed && !denied) {
          unsettled.delete(pendingId);
          setDecision(pendingId, waiting);
        }
        decisionJobs.delete(pendingId);
      }
    })();
    decisionJobs.set(pendingId, job);
    return job;
  };
  const unsubscribeVisibility = env.onVisibilityChange(() => {
    if (env.visible()) { stale(); void refresh(true); }
    schedule();
  });
  const resource: QuestionSetResource = {
    getSnapshot: () => snapshot,
    subscribe(listener) { if (disposed) return () => {}; listeners.add(listener); return () => { listeners.delete(listener); }; },
    refresh: () => refresh(true),
    answer: (pendingId, answers) => decide(pendingId, "answer", answers),
    dismiss: (pendingId) => decide(pendingId, "dismiss"),
    dispose() {
      if (disposed) return;
      disposed = true;
      lifetime.abort();
      if (timer !== undefined) env.clearTimeout(timer);
      unsubscribeVisibility();
      listeners.clear();
      places.clear();
      unsettled.clear();
      // Preserve a stable detached empty snapshot for references retained by a
      // view while React swaps to the next authority-scoped resource.
      snapshot = { scopeKey: input.scopeKey, status: "unavailable", sets: [], closed: [], decisions: table(), notices: [], slots: table(), readOnly: true };
    },
  };
  schedule();
  void refresh(false);
  return resource;
}
