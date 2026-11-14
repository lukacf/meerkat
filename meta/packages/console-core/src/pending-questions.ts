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
/** A decision the gateway refused: nothing was sent to the agent. */
export interface QuestionSetRefusal {
  kind: "access_denied" | "approver_unauthenticated" | "refused";
  /** The reason in words for the person answering. */
  words: string;
  /** The gateway's own message. */
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
 * and it did so after a send this console could not confirm
 * (`sent_unconfirmed`). */
export type QuestionSetNoticeKind = "gone" | "sent_unconfirmed";
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
   * the caller is no signed-in person (only a person answers). */
  readOnlyReason?: "sign_in";
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
const text = (value: unknown) => typeof value === "string" && value.trim() ? value : undefined;
const millis = (value: unknown) => typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= 8_640_000_000_000_000 ? value : undefined;
const record = (value: unknown) => value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;

function normalizeQuestion(value: unknown): Question | null {
  const raw = record(value);
  const id = text(raw?.id), prompt = text(raw?.prompt);
  if (!raw || !id || !prompt || !Array.isArray(raw.choices) || raw.choices.length === 0) return null;
  const choices: QuestionChoice[] = [];
  for (const entry of raw.choices) {
    const choice = record(entry);
    const choiceId = text(choice?.id), label = text(choice?.label);
    if (!choiceId || !label || choices.some((known) => known.id === choiceId)) return null;
    choices.push({ id: choiceId, label });
  }
  const recommended = text(raw.recommended);
  return {
    id, prompt, choices, allowFreeText: raw.allow_free_text === true,
    // A recommendation that names no choice of the question is not shown.
    ...(recommended && choices.some((choice) => choice.id === recommended) ? { recommended } : {}),
  };
}

/** A tolerant reader: unknown fields are ignored, and a set this console
 * cannot present whole (no id, asking agent, digest or readable questions)
 * is dropped. Never throws. */
export function normalizeQuestionSet(value: unknown): QuestionSet | null {
  const raw = record(value);
  const pendingId = text(raw?.pending_id), identity = text(record(raw?.origin)?.identity), subjectDigest = text(raw?.subject_digest);
  if (!raw || !pendingId || !identity || !subjectDigest || !Array.isArray(raw.questions) || raw.questions.length === 0) return null;
  const questions: Question[] = [];
  for (const entry of raw.questions) {
    const question = normalizeQuestion(entry);
    if (!question || questions.some((known) => known.id === question.id)) return null;
    questions.push(question);
  }
  const title = text(raw.title), createdAtMs = millis(raw.created_at_ms), deadlineAtMs = millis(raw.deadline_at_ms);
  return {
    pendingId, origin: { identity }, ...(title ? { title } : {}), questions,
    ...(createdAtMs !== undefined ? { createdAtMs } : {}), ...(deadlineAtMs !== undefined ? { deadlineAtMs } : {}),
    subjectDigest,
  };
}

export function utf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).length;
}

/** The answers of the set's own questions, in the set's order, as the wire
 * carries them. Free text goes without the whitespace at its ends. */
function wireAnswers(set: QuestionSet, answers: QuestionSetAnswers): QuestionSetWireAnswer[] {
  return set.questions.flatMap((question): QuestionSetWireAnswer[] => {
    const answer = Object.prototype.hasOwnProperty.call(answers, question.id) ? answers[question.id] : undefined;
    if (!answer) return [];
    return [{
      question_id: question.id,
      answer: answer.kind === "choice" ? { kind: "choice", choice_id: answer.choiceId } : { kind: "free_text", text: answer.text.trim() },
    }];
  });
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

/** The gateway's decision refusals in words. The kinds are the approvals'
 * own (`access_denied`, `approver_unauthenticated`); every other invalid
 * request reads as one sentence. */
export const QUESTION_SET_REFUSAL_WORDS: Readonly<Record<QuestionSetRefusal["kind"], string>> = {
  approver_unauthenticated: "Sign in as yourself to answer",
  access_denied: "You cannot answer for this agent",
  refused: "This question set is no longer open, or the answer was not accepted.",
};
type RpcFailure = { httpStatus?: unknown; rpcError?: { code?: unknown; message?: unknown; data?: unknown }; responseRpcError?: { code?: unknown; message?: unknown; data?: unknown } };
function rpcFailure(error: unknown): { code?: unknown; kind: string; message?: string } {
  const failure = error as RpcFailure | null;
  const rpc = failure?.rpcError ?? failure?.responseRpcError;
  const data = record(rpc?.data);
  const kind = typeof data?.kind === "string" ? data.kind : typeof data?.error === "string" ? data.error : "";
  return { code: rpc?.code, kind, message: text(rpc?.message) };
}
const isDenied = (error: unknown) => {
  const { code, kind } = rpcFailure(error);
  return (error as RpcFailure | null)?.httpStatus === 403 || code === -32030 || kind === "access_denied";
};
/** A refusal the gateway states (nothing was decided), or null when the
 * outcome is uncertain. */
export function questionSetRefusal(error: unknown): QuestionSetRefusal | null {
  const { code, kind, message } = rpcFailure(error);
  const refusal: QuestionSetRefusal["kind"] | null = kind === "approver_unauthenticated" || code === -32057 ? "approver_unauthenticated"
    : isDenied(error) ? "access_denied"
      : code === -32602 ? "refused" : null;
  return refusal ? { kind: refusal, words: QUESTION_SET_REFUSAL_WORDS[refusal], ...(message ? { detail: message } : {}) } : null;
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
    const place = Object.prototype.hasOwnProperty.call(snapshot.slots, set.pendingId) ? snapshot.slots[set.pendingId] : undefined;
    return place === undefined ? {} : { slot: place };
  };
  return [
    ...snapshot.sets.filter(mine).map((set) => ({ set, decision: snapshot.decisions[set.pendingId], ...slot(set) })),
    ...snapshot.closed.filter((entry) => mine(entry.set)).map((entry) => ({ set: entry.set, decision: snapshot.decisions[entry.set.pendingId], closed: entry.outcome, ...slot(entry.set) })),
  ];
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
    scopeKey: input.scopeKey, status: "loading", sets: [], closed: [], decisions: {}, notices: [], slots: {}, readOnly: input.readOnly === true,
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
  const decisionJobs = new Map<string, Promise<void>>();
  // The place of each card on this page: counted here, never read from a set.
  const places = new Map<string, number>();
  let lastPlace = 0;
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
    publish({ sets: [], closed: [], decisions: {}, notices: [], slots: {}, status, readOnly: true, error: error === undefined ? undefined : errorText(error) });
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
      decisions: Object.fromEntries(Object.entries(decisions).filter(([id]) => kept.has(id))),
      slots: Object.fromEntries(places),
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
          lines.push({ slot, kind: snapshot.decisions[old.pendingId]?.phase === "failed" ? "sent_unconfirmed" : "gone", atMs: now });
        }
        for (const set of sets) if (!places.has(set.pendingId)) places.set(set.pendingId, ++lastPlace);
        const closed = snapshot.closed.filter((old) => !ids.has(old.set.pendingId));
        publish({
          sets, closed, ...keptFor(sets, closed, snapshot.decisions),
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
    const kept = new Set([id, ...snapshot.sets.map((set) => set.pendingId), ...snapshot.closed.map((entry) => entry.set.pendingId)]);
    publish({ decisions: Object.fromEntries(Object.entries({ ...snapshot.decisions, [id]: value }).filter(([key]) => kept.has(key))) });
  };
  const decide = (pendingId: string, action: "answer" | "dismiss", answers?: QuestionSetAnswers): Promise<void> => {
    if (disposed) return Promise.resolve();
    const existing = decisionJobs.get(pendingId);
    if (existing) return existing;
    const set = snapshot.sets.find((candidate) => candidate.pendingId === pendingId);
    if (snapshot.readOnly || denied || snapshot.status !== "ready" || !set
      || (action === "answer" && (!answers || !questionSetAnswersSendable(set, answers)))) {
      return Promise.resolve();
    }
    const previous = snapshot.decisions[pendingId];
    const sent = action === "answer" ? { answers } : {};
    ++decisionGeneration;
    setDecision(pendingId, { phase: "submitting", action, ...sent });
    const job = (async () => {
      // A refusal that does not say whether the set is still there: the
      // gateway answers an id that is not pending the same way as an answer
      // it does not accept. The next read says which.
      let unsettled: QuestionSetRefusal | undefined;
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
          publish({ readOnly: true });
          // A refused retry says nothing about an earlier dispatched attempt.
          setDecision(pendingId, previous?.phase === "failed" ? previous : {
            phase: "unavailable", action, error: "Answers are unavailable with current access",
          });
        } else if (refusal?.kind === "refused") {
          // The set reads as sending until the read has said whether it is
          // still listed; it is never shown as refused before that.
          unsettled = refusal;
        } else {
          setDecision(pendingId, refusal
            ? { phase: "refused", action, ...sent, refusal, error: refusal.words }
            : { phase: "failed", action, ...sent, error: errorText(error) });
        }
      } finally {
        ++decisionGeneration;
        if (!disposed && !denied) await refresh(true);
        // Still listed (or the read failed and the last list stands): the
        // answer itself was not accepted. No longer listed: the read took the
        // card and everything of the set with it.
        if (unsettled && !disposed && !denied && snapshot.sets.some((candidate) => candidate.pendingId === pendingId)) {
          setDecision(pendingId, { phase: "refused", action, ...sent, refusal: unsettled, error: unsettled.words });
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
      // Preserve a stable detached empty snapshot for references retained by a
      // view while React swaps to the next authority-scoped resource.
      snapshot = { scopeKey: input.scopeKey, status: "unavailable", sets: [], closed: [], decisions: {}, notices: [], slots: {}, readOnly: true };
    },
  };
  schedule();
  void refresh(false);
  return resource;
}
