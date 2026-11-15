import * as React from "react";
import {
  FREE_TEXT_MAX_BYTES,
  QUESTION_SET_REFUSAL_WORDS,
  questionSetAnswersOffered,
  questionSetAnswersSendable,
  questionSetEntries,
  questionSetShownWhole,
  utf8ByteLength,
} from "../../../console-core/src/pending-questions";
import type {
  ClosedQuestionSetOutcome,
  Question,
  QuestionAnswer,
  QuestionSet,
  QuestionSetAnswers,
  QuestionSetDecisionState,
  QuestionSetNoticeKind,
  QuestionSetSnapshot,
} from "../../../console-core/src/pending-questions";
import {
  AgentAvatar,
  Button,
  Chip,
  KeyValueList,
  KitGlyph,
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
  /** Whether the list is not ready because its last read failed (the
   *  snapshot holds that read's error), not only because it has not been
   *  read for a while. */
  readFailed?: boolean;
  decision?: QuestionSetDecisionState;
  /** Set once this console answered or dismissed the set. */
  closed?: ClosedQuestionSetOutcome;
  /** The card's place on this page, for what the card remembers of itself
   *  (whether its Technical details are open). */
  slot?: number;
  readOnly?: boolean;
  /** Why the card is read-only, when known: `sign_in` names what lets the
   *  caller answer (only a signed-in person answers). */
  readOnlyReason?: QuestionSetSnapshot["readOnlyReason"];
  onAnswer(pendingId: string, answers: QuestionSetAnswers): void | Promise<void>;
  onDismiss(pendingId: string): void | Promise<void>;
  /** Told as the card leaves the screen while the focus is inside it, with
   *  the card's place: the list moves the focus to the line it shows for
   *  the cards that left. */
  onLeaveFocused?(slot: number): void;
}

/** The question sets a host shows and how it answers them: one snapshot
 * feeds the inbox and every conversation. */
export interface QuestionSetsView {
  snapshot: QuestionSetSnapshot;
  onAnswer(pendingId: string, answers: QuestionSetAnswers): void | Promise<void>;
  onDismiss(pendingId: string): void | Promise<void>;
  /** Reads the question sets again: offered when the first read failed. */
  onRefresh?(): void | Promise<void>;
}

const QuestionSetsContext = React.createContext<QuestionSetsView | undefined>(undefined);
/** Provides the question sets to the conversations below it. */
export const QuestionSetsProvider = QuestionSetsContext.Provider;
export function useQuestionSetsView(): QuestionSetsView | undefined {
  return React.useContext(QuestionSetsContext);
}

type QuestionSetCardState = "pending" | "submitting" | "stale" | "failed" | "answered" | "dismissed" | "refused" | "read_only";

const STATE_WORDS: Record<Exclude<QuestionSetCardState, "read_only">, { text: string; tone: Tone }> = {
  pending: { text: "Answer needed", tone: "warn" },
  submitting: { text: "Sending", tone: "info" },
  stale: { text: "May be out of date", tone: "warn" },
  failed: { text: "Not confirmed", tone: "crit" },
  answered: { text: "Answered", tone: "ok" },
  dismissed: { text: "Dismissed", tone: "neutral" },
  // The gateway refused the reply and the set is still listed. One label
  // for every refusal: each sentence under it says the reply was not
  // accepted or not taken, and not every one of them can say that nothing
  // left the browser.
  refused: { text: "Not accepted", tone: "crit" },
};

/** What the card says under who asked: what to do, where the answers go,
 * what happens next and by when. The asking agent is named. `deadlineAtMs`
 * is the gateway's deadline while it is still ahead on this clock: past it
 * the card says nothing of a timeout, which is the gateway's to declare.
 * `readFailed` says whether the list's last read failed. `blocked` is why
 * this caller cannot reply on this card (null when they can): a sentence
 * promises an answer or a resend only to a caller who can send one. */
function stateSentence(
  state: Exclude<QuestionSetCardState, "refused" | "read_only">, agent: string, action: "answer" | "dismiss" | undefined,
  deadlineAtMs: number | undefined, readFailed: boolean, blocked: string | null,
): string {
  switch (state) {
    case "pending":
      return `Pick one choice per question, then send. Your answers go to ${agent}, which continues with them; a question you leave out reaches it as unanswered.`
        + (deadlineAtMs !== undefined ? ` If nobody answers by ${formatAbsoluteTime(deadlineAtMs)}, ${agent} is told that the questions timed out.` : "");
    case "submitting":
      return action === "dismiss" ? `Telling ${agent} that no answer is coming.` : `Sending your answers to ${agent}.`;
    case "stale":
      // Out of date for one of two reasons, and the card says which: a read
      // failed, or no list was applied for a while (a reply is on its way,
      // or the tab just came back).
      return readFailed
        ? `The Console could not read the questions again just now, so this card may be out of date. It keeps trying${blocked === null ? "; you can answer once it has read them" : ""}.`
        : `The Console has not read the questions for a while, so this card may be out of date. It is reading them again${blocked === null ? "; you can answer once it has" : ""}.`;
    case "failed": {
      // In the words of what was sent, with a resend only where one can be sent.
      const what = action === "dismiss" ? "The Console could not confirm that the dismissal arrived." : "The Console could not confirm that your answer arrived.";
      if (blocked !== null) return `${what} ${blocked}`;
      return `${what} If this question is still listed after the next refresh, ${action === "dismiss" ? "dismiss it again" : "send your answer again"}.`;
    }
    case "answered":
      return `Your answers were sent to ${agent}, which continues with them.`;
    case "dismissed":
      return `Dismissed: no answer is coming, and ${agent} was told so.`;
  }
}

/** Why nobody can answer on this card now, and what to do about it: the
 * chip and the sentence, by the snapshot's read-only reason. */
const READ_ONLY_WORDS = {
  sign_in: { chip: "Sign in to answer", sentence: "Sign in as yourself to answer. Only a signed-in person can answer an agent's questions." },
  no_permission: { chip: "No permission", sentence: "You no longer have permission to answer questions here. Ask an administrator for the permission to decide." },
  read_only: { chip: "Read only", sentence: "You can read these questions but not answer them: your access to this Console is read-only." },
} as const;

/** The one line a card that left the screen leaves where it was. It names
 * nothing of the set, and says no more than the list does: the set is no
 * longer listed to this caller (it may still wait for someone else). */
const NOTICE_WORDS: Record<QuestionSetNoticeKind, string> = {
  gone: "This question is no longer listed here.",
  sent_unconfirmed: "This question is no longer listed here. The Console could not confirm whether your reply arrived.",
  not_open: QUESTION_SET_REFUSAL_WORDS.not_open,
  expired: QUESTION_SET_REFUSAL_WORDS.expired,
};

/** Show the byte counter from here on: near the limit, not before. */
const FREE_TEXT_COUNTER_FROM = 200;
const SOMETHING_ELSE = "Something else";
/** How long after Send or Dismiss the card still takes the focus for its
 * sentence: the state follows the press at once, so anything later is not
 * that press. */
const ACTED_FOCUS_MS = 5_000;

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
  const group = React.useRef<HTMLFieldSetElement>(null);
  const freeText = answer?.kind === "free_text" ? answer.text : null;
  const bytes = freeText === null ? 0 : utf8ByteLength(freeText.trim());
  const tooLong = bytes > FREE_TEXT_MAX_BYTES;
  const counted = bytes >= FREE_TEXT_COUNTER_FROM;
  const more = question.omittedChoices ?? 0;
  return (
    <fieldset ref={group} className="cc-question" disabled={disabled} data-testid={`question:${setId}:${question.id}`}>
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
      {more > 0 ? (
        <p className="cc-question__more">{`${more} more ${more === 1 ? "choice is" : "choices are"} not shown.`}</p>
      ) : null}
      {answer && !disabled ? (
        <button
          type="button"
          className="cc-question__clear"
          aria-label={`Clear the answer to: ${question.prompt}`}
          onClick={() => {
            onChange(undefined);
            // Clear goes with the answer: focus moves to the question's
            // first choice instead of being lost.
            group.current?.querySelector<HTMLInputElement>("input[type='radio']")?.focus();
          }}
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

/** The set's deadline, which is the gateway's. Before it on this clock, a
 * countdown; past it, only the time: this clock ends no set, and a set
 * stays answerable while the gateway lists it. */
function AnswerByChip({ deadlineAtMs, now, testId }: { deadlineAtMs: number; now: number; testId: string }) {
  if (now < deadlineAtMs) return <DeadlineChip deadlineAtMs={deadlineAtMs} now={now} testId={testId} />;
  return (
    <span className="cc-countdown cc-countdown--chip cc-approval__deadline" data-tone="neutral" data-testid={testId}>
      <KitGlyph name="clock" className="cc-countdown__glyph" />
      <time dateTime={new Date(deadlineAtMs).toISOString()}>{`Answer by ${formatAbsoluteTime(deadlineAtMs)}`}</time>
    </span>
  );
}

/** One set of questions an agent asked a person, in the approval card's
 * family. The same set and decision owner can feed an inbox and several
 * panes; each card keeps its own draft until it is sent. */
export function QuestionSetCard({ set, resourceStatus, readFailed = false, decision, closed, slot, readOnly = false, readOnlyReason, onAnswer, onDismiss, onLeaveFocused }: QuestionSetCardProps) {
  const directory = useAgentDirectory();
  const navigation = useConsoleNavigation();
  const now = useApprovalClock([set.deadlineAtMs]);
  const [draft, setDraft] = React.useState<QuestionSetAnswers>({});
  const identity = set.origin.identity;
  const agentName = agentDisplayName(identity, directory) || "an agent";
  const refusal = decision?.phase === "refused" ? decision.refusal : undefined;
  // A set the reader cut is read-only for everyone: a reply would send back
  // the digest of the whole set, which the card did not show.
  const partial = !questionSetShownWhole(set);
  // The state is the gateway's and this console's own decisions: never the
  // browser clock. A list that could not be read again comes before what an
  // earlier reply said of the set.
  const state: QuestionSetCardState = closed === "answered" || decision?.phase === "answered" ? "answered"
    : closed === "dismissed" || decision?.phase === "dismissed" ? "dismissed"
      : decision?.phase === "submitting" ? "submitting"
        : decision?.phase === "failed" ? "failed"
          : resourceStatus !== "ready" ? "stale"
            : refusal ? "refused"
              : readOnly || partial ? "read_only"
                : "pending";
  const locked = partial
    ? { chip: READ_ONLY_WORDS.read_only.chip, sentence: `This set of questions is larger than the Console shows, so it cannot be answered here. Reply to ${agentName} in its chat instead.` }
    : READ_ONLY_WORDS[readOnlyReason ?? "read_only"];
  const words = state === "read_only" ? { text: locked.chip, tone: "locked" as Tone } : STATE_WORDS[state];
  const sentence = state === "refused" ? refusal?.words ?? "" : state === "read_only" ? locked.sentence
    : stateSentence(
      state, agentName, decision?.action, set.deadlineAtMs !== undefined && now < set.deadlineAtMs ? set.deadlineAtMs : undefined,
      readFailed, partial || readOnly ? locked.sentence : null,
    );
  const open = !closed && state !== "answered" && state !== "dismissed";
  // A person changes the draft while the set waits, after an uncertain send,
  // and after a refusal the set outlives (they may correct and send again),
  // unless the cards are read-only or the set is not shown whole.
  const editable = open && !readOnly && !partial && (state === "pending" || state === "failed" || state === "refused");
  // What the set offers now: a set that changed may have dropped a question
  // or a choice the draft still holds.
  const answers = questionSetAnswersOffered(set, draft);
  const sendable = editable && questionSetAnswersSendable(set, answers);
  const title = set.title || `Questions from ${agentName}`;
  const waiting = open && set.createdAtMs !== undefined ? formatDuration(Math.max(0, now - set.createdAtMs)) : "";
  const openChat = navigation?.openAgent ? () => navigation.openAgent?.(identity, "chat") : null;
  const change = (questionId: string, answer: QuestionAnswer | undefined) => setDraft((current) => {
    const { [questionId]: _previous, ...rest } = current;
    return answer ? { ...rest, [questionId]: answer } : rest;
  });
  // Send and Dismiss disable themselves once pressed. Focus then goes to the
  // sentence that says what is happening, on the card that was acted on.
  const sentenceRef = React.useRef<HTMLElement | null>(null);
  const actedAtMs = React.useRef<number | null>(null);
  React.useEffect(() => {
    const acted = actedAtMs.current;
    actedAtMs.current = null;
    if (state === "submitting" && acted !== null && Date.now() - acted < ACTED_FOCUS_MS) sentenceRef.current?.focus();
  }, [state]);
  // A card that leaves the screen while the focus is inside it tells its
  // list, which moves the focus to the line it shows for the cards that
  // left. (The
  // cleanup of a layout effect runs before the card's nodes are removed.)
  const cardRef = React.useRef<HTMLElement | null>(null);
  const leave = React.useRef({ slot, onLeaveFocused });
  leave.current = { slot, onLeaveFocused };
  React.useLayoutEffect(() => () => {
    const card = cardRef.current;
    const { slot: place, onLeaveFocused: tell } = leave.current;
    if (card && place !== undefined && tell && typeof document !== "undefined" && card.contains(document.activeElement)) tell(place);
  }, []);
  const omitted = set.omittedQuestions ?? 0;
  const more = [
    omitted > 0 ? `${omitted} more ${omitted === 1 ? "question is" : "questions are"} not shown.` : "",
    set.shortened ? "Some text is too long and is shown shortened." : "",
  ].filter(Boolean).join(" ");
  return (
    <article ref={cardRef} className="cc-approval cc-question-set" data-state={state} data-testid={`question-set:${set.pendingId}`}>
      <header className="cc-approval__header">
        <div className="cc-approval__heading">
          <strong className="cc-approval__title">{title}</strong>
          <span className="cc-approval__status" role="status">
            <Chip tone={words.tone} mark variant={state === "pending" ? "quiet" : undefined}>{words.text}</Chip>
          </span>
        </div>
        {open && set.deadlineAtMs !== undefined ? (
          <AnswerByChip deadlineAtMs={set.deadlineAtMs} now={now} testId={`question-set-deadline:${set.pendingId}`} />
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
        <p ref={sentenceRef as React.Ref<HTMLParagraphElement>} tabIndex={-1} className="cc-question-set__hint cc-question-set__sentence" data-testid={`question-set-sentence:${set.pendingId}`}>{sentence}</p>
      ) : (
        <Notice
          tone={state === "failed" || state === "refused" ? "crit" : state === "stale" ? "warn" : state === "answered" ? "ok" : state === "read_only" ? "locked" : "neutral"}
          role={state === "failed" || state === "refused" ? "alert" : undefined}
        >
          <span ref={sentenceRef as React.Ref<HTMLSpanElement>} tabIndex={-1} className="cc-question-set__sentence" data-testid={`question-set-sentence:${set.pendingId}`}>{sentence}</span>
          {state === "refused" && refusal?.detail ? (
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
              answer={answers[question.id]}
              disabled={!editable}
              onChange={(answer) => change(question.id, answer)}
            />
          ))}
          {more ? <p className="cc-question__more" data-testid={`question-set-more:${set.pendingId}`}>{more}</p> : null}
        </div>
      )}
      <footer className="cc-approval__footer">
        {open ? (
          <div className="cc-approval__actions">
            <Button
              variant="primary"
              disabled={!sendable}
              data-testid={`question-set-send:${set.pendingId}`}
              onClick={() => { actedAtMs.current = Date.now(); void onAnswer(set.pendingId, answers); }}
            >
              Send answers
            </Button>
            <Button
              disabled={!editable}
              data-testid={`question-set-dismiss:${set.pendingId}`}
              onClick={() => { actedAtMs.current = Date.now(); void onDismiss(set.pendingId); }}
            >
              Dismiss
            </Button>
          </div>
        ) : null}
        <RowDetails part={`question-set-details:${slot ?? "card"}`} className="cc-approval__details cc-tech" data-testid={`question-set-details:${set.pendingId}`}>
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
 * that has not answered yet show nothing. A card whose set the gateway no
 * longer lists is gone, and where this list showed it one neutral line
 * stays for a while: the newest, when several cards left. */
export function QuestionSetList({ view, identity, readOnly = false, className, testId, onContent }: {
  view?: QuestionSetsView;
  /** Only the sets this agent asked. */
  identity?: string;
  /** The host's own read-only mode, on top of the snapshot's. */
  readOnly?: boolean;
  className?: string;
  testId?: string;
  /** Told whether this list shows anything (a card, its own line, the
   *  failed first read), whenever that changes: a host lays out what is
   *  beside the list by what the list shows, not by what the snapshot holds. */
  onContent?(shows: boolean): void;
}): React.JSX.Element | null {
  // The places of the cards this list has shown: numbers the console
  // counts, so the list can tell which lines are for cards it showed
  // without keeping anything of a set.
  const shown = React.useRef(new Set<number>());
  const entries = view ? questionSetEntries(view.snapshot, identity) : [];
  const notices = view?.snapshot.notices;
  React.useEffect(() => {
    // The places of the cards on screen now, and of the cards this list
    // showed whose lines are still kept: nothing older.
    const places = new Set<number>();
    for (const entry of entries) if (entry.slot !== undefined) places.add(entry.slot);
    for (const line of notices ?? []) if (shown.current.has(line.slot)) places.add(line.slot);
    shown.current = places;
  });
  const notice = view?.snapshot.notices.find((line) => shown.current.has(line.slot));
  // The first read failed: nothing was ever listed. The inbox's list says so
  // once, with a way to try again; a conversation does not repeat it.
  const unread = view !== undefined && identity === undefined && view.snapshot.status === "unavailable" && view.snapshot.error !== undefined;
  const shows = entries.length > 0 || notice !== undefined || unread;
  const told = React.useRef<boolean | null>(null);
  const tell = React.useRef(onContent);
  tell.current = onContent;
  React.useEffect(() => {
    if (told.current !== shows) { told.current = shows; onContent?.(shows); }
  });
  // A list that is no longer on screen shows nothing.
  React.useEffect(() => () => { if (told.current) { told.current = null; tell.current?.(false); } }, []);
  // A card that held the focus left: the focus goes to the line this list
  // shows. That is the card's own line, or, when several cards left in one
  // read, the one line shown for them. With no line it goes to the list
  // itself, where the host gave it an element; never to the page.
  const lineRef = React.useRef<HTMLDivElement | null>(null);
  const listRef = React.useRef<HTMLDivElement | null>(null);
  const leftFocused = React.useRef(false);
  React.useEffect(() => {
    const left = leftFocused.current;
    leftFocused.current = false;
    if (left) (lineRef.current ?? listRef.current)?.focus();
  });
  if (!view || !shows) return null;
  const { snapshot } = view;
  const cards = entries.map((entry) => (
    <QuestionSetCard
      key={entry.set.pendingId}
      set={entry.set}
      resourceStatus={snapshot.status}
      readFailed={snapshot.status !== "ready" && snapshot.error !== undefined}
      decision={entry.decision}
      closed={entry.closed}
      slot={entry.slot}
      readOnly={readOnly || snapshot.readOnly}
      readOnlyReason={readOnly ? undefined : snapshot.readOnlyReason}
      onAnswer={view.onAnswer}
      onDismiss={view.onDismiss}
      onLeaveFocused={() => { leftFocused.current = true; }}
    />
  ));
  const line = notice ? (
    <div key="notice" ref={lineRef} tabIndex={-1} className="cc-question-set__line" data-testid="question-set-line">
      <Notice tone="neutral" role="status" className="cc-question-set__notice" testId="question-set-notice">{NOTICE_WORDS[notice.kind]}</Notice>
    </div>
  ) : null;
  const failed = unread ? (
    <Notice
      key="unavailable" tone="warn" role="status" className="cc-question-set__notice" testId="question-set-unavailable"
      action={view.onRefresh ? <Button size="sm" onClick={() => { void view.onRefresh?.(); }}>Try again</Button> : undefined}
    >
      The questions agents asked could not be loaded. The Console keeps trying; you can also try now.
    </Notice>
  ) : null;
  return className || testId ? <div ref={listRef} tabIndex={-1} className={className} data-testid={testId}>{failed}{line}{cards}</div> : <>{failed}{line}{cards}</>;
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
