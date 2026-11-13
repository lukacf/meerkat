import type { ApprovalResourceEnvironment } from "./pending-approvals";

export const FREE_TEXT_MAX_BYTES = 280;
export interface QuestionChoice {
  id: string;
  label: string;
}
export interface Question {
  id: string;
  prompt: string;
  choices: readonly QuestionChoice[];
  allowFreeText: boolean;
  recommended?: string;
}
export interface QuestionSet {
  pendingId: string;
  origin: { identity: string };
  title?: string;
  questions: readonly Question[];
  createdAtMs?: number;
  deadlineAtMs?: number;
  subjectDigest: string;
}
export type QuestionAnswer = { kind: "choice"; choiceId: string } | { kind: "free_text"; text: string };
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
export interface QuestionSetRefusal {
  kind: "access_denied" | "approver_unauthenticated" | "refused";
  words: string;
  detail?: string;
}
export interface QuestionSetDecisionState {
  phase: "submitting" | "answered" | "dismissed" | "failed" | "refused" | "unavailable";
  action: "answer" | "dismiss";
  answers?: QuestionSetAnswers;
  decidedAtMs?: number;
  error?: string;
  refusal?: QuestionSetRefusal;
}
export type ClosedQuestionSetOutcome = "answered" | "dismissed" | "timed_out" | "ended";
export interface ClosedQuestionSet {
  set: QuestionSet;
  outcome: ClosedQuestionSetOutcome;
  closedAtMs: number;
}
export interface QuestionSetSnapshot {
  scopeKey: string;
  status: "loading" | "ready" | "stale" | "unavailable" | "forbidden" | "unsupported";
  sets: readonly QuestionSet[];
  closed: readonly ClosedQuestionSet[];
  decisions: Readonly<Record<string, QuestionSetDecisionState>>;
  readOnly: boolean;
  readOnlyReason?: "sign_in";
  updatedAtMs?: number;
  error?: string;
}
export interface QuestionSetEntry {
  set: QuestionSet;
  decision?: QuestionSetDecisionState;
  closed?: ClosedQuestionSetOutcome;
}
export interface QuestionSetResource {
  getSnapshot(): QuestionSetSnapshot;
  subscribe(listener: () => void): () => void;
  refresh(): Promise<void>;
  answer(pendingId: string, answers: QuestionSetAnswers): Promise<void>;
  dismiss(pendingId: string): Promise<void>;
  dispose(): void;
}

export function normalizeQuestionSet(_value: unknown): QuestionSet | null {
  return null;
}
export function utf8ByteLength(_text: string): number {
  return 0;
}
export function questionSetAnswersSendable(_set: QuestionSet, _answers: QuestionSetAnswers): boolean {
  return false;
}
export function questionSetAnswerParams(_set: QuestionSet, _answers: QuestionSetAnswers): QuestionSetAnswerParams {
  return { pending_id: "", decision: "approve", presented_digest: "", answers: [] };
}
export function questionSetDismissParams(_set: QuestionSet): QuestionSetDismissParams {
  return { pending_id: "", decision: "reject", presented_digest: "" };
}
export function questionSetRefusal(_error: unknown): QuestionSetRefusal | null {
  return null;
}
export function questionSetEntries(_snapshot: QuestionSetSnapshot, _identity?: string): QuestionSetEntry[] {
  return [];
}
export function createQuestionSetResource(input: {
  scopeKey: string;
  load(signal: AbortSignal): Promise<unknown>;
  decide(params: QuestionSetAnswerParams | QuestionSetDismissParams, signal: AbortSignal): Promise<unknown>;
  readOnly?: boolean;
  readOnlyReason?: "sign_in";
  environment?: ApprovalResourceEnvironment;
}): QuestionSetResource {
  const snapshot: QuestionSetSnapshot = { scopeKey: input.scopeKey, status: "loading", sets: [], closed: [], decisions: {}, readOnly: true };
  return {
    getSnapshot: () => snapshot,
    subscribe: () => () => {},
    refresh: () => Promise.resolve(),
    answer: () => Promise.resolve(),
    dismiss: () => Promise.resolve(),
    dispose() {},
  };
}
