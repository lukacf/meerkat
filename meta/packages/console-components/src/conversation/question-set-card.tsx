import * as React from "react";
import {
  FREE_TEXT_MAX_BYTES,
  QUESTION_SET_REFUSAL_WORDS,
  questionSetAnswersSendable,
  questionSetEntries,
  utf8ByteLength,
} from "../../../console-core/src/pending-questions";
import type {
  ClosedQuestionSetOutcome,
  Question,
  QuestionAnswer,
  QuestionSet,
  QuestionSetAnswers,
  QuestionSetDecisionState,
  QuestionSetSnapshot,
} from "../../../console-core/src/pending-questions";
import {
  AgentAvatar,
  Button,
  Chip,
  KeyValueList,
  Notice,
  agentDisplayName,
  cx,
  formatAbsoluteTime,
  formatDuration,
  useAgentDirectory,
  useConsoleNavigation,
} from "../panel-kit";
import type { Tone } from "../panel-kit";
import { DeadlineChip, useApprovalClock } from "./approval-card";
import { RowDetails } from "./presentation-policy";

export interface QuestionSetCardProps {
  set: QuestionSet;
  resourceStatus: QuestionSetSnapshot["status"];
  decision?: QuestionSetDecisionState;
  /** How the set ended, once it is no longer listed as open. */
  closed?: ClosedQuestionSetOutcome;
  readOnly?: boolean;
  /** Why the card is read-only, when known: `sign_in` names what lets the
   *  caller answer (only a signed-in person answers). */
  readOnlyReason?: QuestionSetSnapshot["readOnlyReason"];
  onAnswer(pendingId: string, answers: QuestionSetAnswers): void | Promise<void>;
  onDismiss(pendingId: string): void | Promise<void>;
}

/** The question sets a host shows and how it answers them: one snapshot
 * feeds the inbox and every conversation. */
export interface QuestionSetsView {
  snapshot: QuestionSetSnapshot;
  onAnswer(pendingId: string, answers: QuestionSetAnswers): void | Promise<void>;
  onDismiss(pendingId: string): void | Promise<void>;
}

const QuestionSetsContext = React.createContext<QuestionSetsView | undefined>(undefined);
/** Provides the question sets to the conversations below it. */
export const QuestionSetsProvider = QuestionSetsContext.Provider;
export function useQuestionSetsView(): QuestionSetsView | undefined {
  return React.useContext(QuestionSetsContext);
}

type QuestionSetCardState = "pending" | "submitting" | "stale" | "failed" | "answered" | "dismissed" | "timed_out" | "not_permitted";

const STATE_WORDS: Record<QuestionSetCardState, { text: string; tone: Tone }> = {
  pending: { text: "Answer needed", tone: "warn" },
  submitting: { text: "Sending", tone: "info" },
  stale: { text: "May be out of date", tone: "warn" },
  failed: { text: "Not confirmed", tone: "crit" },
  answered: { text: "Answered", tone: "ok" },
  dismissed: { text: "Dismissed", tone: "neutral" },
  timed_out: { text: "Timed out", tone: "neutral" },
  not_permitted: { text: "Not permitted", tone: "locked" },
};

const SENTENCES = {
  pending: "Pick one choice per question. A question you leave out reaches the agent as unanswered.",
  submitting: "Sending your decision to the agent.",
  stale: "The question sets could not be refreshed. Answers can be sent once they are read again.",
  failed: "The answer may not have been sent. This card updates when the gateway answers again.",
  answered: "Answers sent.",
  dismissed: "Dismissed. The agent was told no answer is coming.",
  timed_out: "This question set timed out before it was answered.",
} satisfies Record<Exclude<QuestionSetCardState, "not_permitted">, string>;

/** Show the byte counter from here on: near the limit, not before. */
const FREE_TEXT_COUNTER_FROM = 200;
const SOMETHING_ELSE = "Something else";

/** One question: its prompt as the group's legend, one native radio per
 * choice (arrow keys and Tab work as in any radio group), the recommended
 * choice marked and never preselected, and, where the agent allowed it, a
 * last choice that reveals a text field. Every word is a React text node. */
function QuestionGroup({ setId, question, answer, disabled, onChange }: {
  setId: string;
  question: Question;
  answer: QuestionAnswer | undefined;
  disabled: boolean;
  onChange(answer: QuestionAnswer | undefined): void;
}) {
  const name = React.useId();
  const counterId = React.useId();
  const freeText = answer?.kind === "free_text" ? answer.text : null;
  const bytes = freeText === null ? 0 : utf8ByteLength(freeText.trim());
  const tooLong = bytes > FREE_TEXT_MAX_BYTES;
  const counted = bytes >= FREE_TEXT_COUNTER_FROM;
  return (
    <fieldset className="cc-question" disabled={disabled} data-testid={`question:${setId}:${question.id}`}>
      <legend className="cc-question__prompt">{question.prompt}</legend>
      <div className="cc-question__choices">
        {question.choices.map((choice) => (
          <label key={choice.id} className="cc-question__choice">
            <input
              type="radio"
              name={name}
              value={choice.id}
              disabled={disabled}
              checked={answer?.kind === "choice" && answer.choiceId === choice.id}
              onChange={() => onChange({ kind: "choice", choiceId: choice.id })}
            />
            <span className="cc-question__label">{choice.label}</span>
            {question.recommended === choice.id ? (
              <Chip tone="info" className="cc-question__recommended">Recommended</Chip>
            ) : null}
          </label>
        ))}
        {question.allowFreeText ? (
          <label className="cc-question__choice">
            <input
              type="radio"
              name={name}
              value=""
              disabled={disabled}
              checked={freeText !== null}
              onChange={() => onChange({ kind: "free_text", text: "" })}
            />
            <span className="cc-question__label">{SOMETHING_ELSE}</span>
          </label>
        ) : null}
      </div>
      {freeText !== null ? (
        <div className="cc-question__free">
          <textarea
            className="cc-input cc-question__free-text"
            rows={2}
            value={freeText}
            disabled={disabled}
            aria-label={`Your answer to: ${question.prompt}`}
            aria-describedby={counted ? counterId : undefined}
            aria-invalid={tooLong || undefined}
            onChange={(event) => onChange({ kind: "free_text", text: event.target.value })}
          />
          {counted ? (
            <span
              id={counterId}
              className="cc-question__count"
              data-tone={tooLong ? "crit" : undefined}
              data-testid={`question-free-count:${setId}:${question.id}`}
            >
              {`${tooLong ? "Too long: " : ""}${bytes} of ${FREE_TEXT_MAX_BYTES} bytes`}
            </span>
          ) : null}
        </div>
      ) : null}
      {answer && !disabled ? (
        <button
          type="button"
          className="cc-question__clear"
          aria-label={`Clear the answer to: ${question.prompt}`}
          onClick={() => onChange(undefined)}
        >
          Clear
        </button>
      ) : null}
    </fieldset>
  );
}

/** What was sent, read-only: each prompt with the chosen label, the
 * person's own words, or "Not answered". */
function AnswerSummary({ set, answers }: { set: QuestionSet; answers: QuestionSetAnswers }) {
  return (
    <dl className="cc-question-set__summary" data-testid={`question-set-summary:${set.pendingId}`}>
      {set.questions.map((question) => {
        const answer = Object.prototype.hasOwnProperty.call(answers, question.id) ? answers[question.id] : undefined;
        const words = !answer ? "Not answered"
          : answer.kind === "free_text" ? answer.text.trim()
            : question.choices.find((choice) => choice.id === answer.choiceId)?.label ?? "Not answered";
        return (
          <React.Fragment key={question.id}>
            <dt className="cc-question-set__asked">{question.prompt}</dt>
            <dd className={cx("cc-question-set__sent", !answer && "is-empty")}>{words}</dd>
          </React.Fragment>
        );
      })}
    </dl>
  );
}

/** One set of questions an agent asked a person, in the approval card's
 * family. The same set and decision owner can feed an inbox and several
 * panes; each card keeps its own draft until it is sent. */
export function QuestionSetCard({ set, resourceStatus, decision, closed, readOnly = false, readOnlyReason, onAnswer, onDismiss }: QuestionSetCardProps) {
  const directory = useAgentDirectory();
  const navigation = useConsoleNavigation();
  const now = useApprovalClock([set.deadlineAtMs]);
  const [draft, setDraft] = React.useState<QuestionSetAnswers>({});
  const identity = set.origin.identity;
  const agentName = agentDisplayName(identity, directory) || "an agent";
  const refusal = decision?.phase === "refused" ? decision.refusal : undefined;
  const state: QuestionSetCardState = closed === "answered" || decision?.phase === "answered" ? "answered"
    : closed === "dismissed" || decision?.phase === "dismissed" ? "dismissed"
      : decision?.phase === "submitting" ? "submitting"
        : closed === "timed_out" || (set.deadlineAtMs !== undefined && now >= set.deadlineAtMs) ? "timed_out"
          : decision?.phase === "failed" ? "failed"
            : refusal || closed === "ended" || readOnly ? "not_permitted"
              : resourceStatus !== "ready" ? "stale"
                : "pending";
  const words = STATE_WORDS[state];
  // Not permitted has three forms: sign in, no access to this agent, and
  // the gateway's refusal of this decision (its own message under Details).
  const refusalKind = state !== "not_permitted" ? null
    : refusal ? refusal.kind
      : closed === "ended" ? "refused"
        : readOnlyReason === "sign_in" ? "approver_unauthenticated" : "access_denied";
  const sentence = refusalKind ? QUESTION_SET_REFUSAL_WORDS[refusalKind] : SENTENCES[state as Exclude<QuestionSetCardState, "not_permitted">];
  const open = !closed && state !== "answered" && state !== "dismissed" && state !== "timed_out";
  // A person changes the draft while the set waits, after an uncertain send,
  // and after a refusal of the answer itself (they may correct and resend).
  const editable = open && !readOnly && (state === "pending" || state === "failed" || (state === "not_permitted" && refusalKind === "refused"));
  const sendable = editable && questionSetAnswersSendable(set, draft);
  const title = set.title || `Questions from ${agentName}`;
  const waiting = open && set.createdAtMs !== undefined ? formatDuration(Math.max(0, now - set.createdAtMs)) : "";
  const openChat = navigation?.openAgent ? () => navigation.openAgent?.(identity, "chat") : null;
  const change = (questionId: string, answer: QuestionAnswer | undefined) => setDraft((current) => {
    const { [questionId]: _previous, ...rest } = current;
    return answer ? { ...rest, [questionId]: answer } : rest;
  });
  return (
    <article className="cc-approval cc-question-set" data-state={state} data-testid={`question-set:${set.pendingId}`}>
      <header className="cc-approval__header">
        <div className="cc-approval__heading">
          <strong className="cc-approval__title">{title}</strong>
          <span className="cc-approval__status" role="status">
            <Chip tone={words.tone} mark variant={state === "pending" ? "quiet" : undefined}>{words.text}</Chip>
          </span>
        </div>
        {open && set.deadlineAtMs !== undefined ? (
          <DeadlineChip deadlineAtMs={set.deadlineAtMs} now={now} testId={`question-set-deadline:${set.pendingId}`} />
        ) : null}
      </header>
      <p className="cc-approval__asked" data-testid={`question-set-asked:${set.pendingId}`}>
        <span className="cc-approval__asker">
          <AgentAvatar name={agentName} size={22} />
          <span>Asked by <span className="cc-approval__actor">{agentName}</span></span>
        </span>
        {openChat ? (
          <span className="cc-approval__origin">
            <button type="button" className="cc-approval__origin-link" onClick={openChat} aria-label={`Open ${agentName}'s chat`}>
              Open the chat
            </button>
          </span>
        ) : null}
        {waiting ? <span className="cc-approval__waiting">{`Waiting ${waiting}`}</span> : null}
      </p>
      {state === "pending" ? (
        <p className="cc-question-set__hint" data-testid={`question-set-sentence:${set.pendingId}`}>{sentence}</p>
      ) : (
        <Notice
          tone={state === "failed" ? "crit" : state === "stale" ? "warn" : state === "answered" ? "ok" : state === "not_permitted" ? "locked" : "neutral"}
          role={state === "failed" || refusal ? "alert" : undefined}
        >
          <span data-testid={`question-set-sentence:${set.pendingId}`}>{sentence}</span>
          {refusalKind === "refused" && refusal?.detail ? (
            <details className="cc-question-set__refusal" data-testid={`question-set-refusal:${set.pendingId}`}>
              <summary>Details</summary>
              <span className="cc-question-set__gateway">{refusal.detail}</span>
            </details>
          ) : null}
        </Notice>
      )}
      {state === "answered" ? (
        decision?.answers ? <AnswerSummary set={set} answers={decision.answers} /> : null
      ) : state === "dismissed" ? null : (
        <div className="cc-question-set__questions">
          {set.questions.map((question) => (
            <QuestionGroup
              key={question.id}
              setId={set.pendingId}
              question={question}
              answer={draft[question.id]}
              disabled={!editable}
              onChange={(answer) => change(question.id, answer)}
            />
          ))}
        </div>
      )}
      <footer className="cc-approval__footer">
        {open ? (
          <div className="cc-approval__actions">
            <Button
              variant="primary"
              disabled={!sendable}
              data-testid={`question-set-send:${set.pendingId}`}
              onClick={() => { void onAnswer(set.pendingId, draft); }}
            >
              Send answers
            </Button>
            <Button
              disabled={!editable}
              data-testid={`question-set-dismiss:${set.pendingId}`}
              onClick={() => { void onDismiss(set.pendingId); }}
            >
              Dismiss
            </Button>
          </div>
        ) : null}
        <RowDetails part={`question-set-details:${set.pendingId}`} className="cc-approval__details cc-tech" data-testid={`question-set-details:${set.pendingId}`}>
          <summary className="cc-tech__summary">Technical details</summary>
          <div className="cc-tech__body">
            <KeyValueList
              items={[
                { key: "request", label: "Request", value: <code>{set.pendingId}</code> },
                { key: "agent", label: "Asking agent", value: <code>{identity}</code> },
                { key: "digest", label: "Presented digest", value: <code>{set.subjectDigest}</code> },
                set.deadlineAtMs !== undefined ? { key: "deadline", label: "Deadline", value: formatAbsoluteTime(set.deadlineAtMs) } : null,
                ...set.questions.map((question, index) => ({
                  key: `question:${question.id}`,
                  label: `Question ${index + 1}`,
                  value: <code>{`${question.id}: ${question.choices.map((choice) => choice.id).join(", ")}`}</code>,
                })),
              ]}
            />
          </div>
        </RowDetails>
      </footer>
    </article>
  );
}

/** The cards of a snapshot, for the inbox (every agent) or one agent's
 * conversation. A gateway without question sets, a denied read and a read
 * that has not answered yet show nothing. */
export function QuestionSetList({ view, identity, readOnly = false, className, testId }: {
  view?: QuestionSetsView;
  /** Only the sets this agent asked. */
  identity?: string;
  /** The host's own read-only mode, on top of the snapshot's. */
  readOnly?: boolean;
  className?: string;
  testId?: string;
}): React.JSX.Element | null {
  if (!view) return null;
  const { snapshot } = view;
  const entries = questionSetEntries(snapshot, identity);
  if (!entries.length) return null;
  const cards = entries.map((entry) => (
    <QuestionSetCard
      key={entry.set.pendingId}
      set={entry.set}
      resourceStatus={snapshot.status}
      decision={entry.decision}
      closed={entry.closed}
      readOnly={readOnly || snapshot.readOnly}
      readOnlyReason={readOnly ? undefined : snapshot.readOnlyReason}
      onAnswer={view.onAnswer}
      onDismiss={view.onDismiss}
    />
  ));
  return className || testId ? <div className={className} data-testid={testId}>{cards}</div> : <>{cards}</>;
}

/** The question sets the conversation's agent asked, from the provider:
 * placed with the conversation's approvals. */
export function ConversationQuestionSets({ identity }: {
  /** Canonical host-authorized target, never a label or actor ID. */
  identity?: string;
}): React.JSX.Element | null {
  const view = useQuestionSetsView();
  if (!view || !identity) return null;
  return <QuestionSetList view={view} identity={identity} />;
}
