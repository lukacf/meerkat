import React from "react";
import { DeadlineChip, useApprovalClock } from "../../../packages/console-components/src/conversation/approval-card";
import {
  Chip,
  KeyValueList,
  TechnicalDetails,
  formatAbsoluteTime,
  humanizeEnum,
} from "../../../packages/console-components/src/panel-kit";
import type { Tone } from "../../../packages/console-components/src/panel-kit";
import { phaseWords, type DecisionCardView } from "./decision-cards";

/** The tone of a wire phase. A phase this console does not know reads
 *  neutral. */
function phaseTone(phase: string): Tone {
  switch (phase) {
    case "open":
      return "warn";
    case "approved":
    case "executed":
      return "ok";
    case "rejected":
    case "execution_refused":
      return "crit";
    default:
      return "neutral";
  }
}

/** One durable decision, read only. A whole card shows who approves it,
 *  who asked, its rendering and where it was presented; the decision is
 *  decided there, never here. A status card shows the phase and the times
 *  only, none of what the decision holds. */
export function DecisionCard({ card, now }: { card: DecisionCardView; now: number }): React.JSX.Element {
  const open = card.phase === "open";
  const presentations = card.presentations ?? [];
  return (
    <article
      className="cc-approval cc-decision"
      data-kind="decision"
      data-phase={card.phase}
      data-status-only={card.statusOnly ? "true" : undefined}
      data-testid={`decision-card:${card.decisionId}`}
    >
      <header className="cc-approval__header">
        <div className="cc-approval__heading">
          <strong className="cc-approval__title">{humanizeEnum(card.kind)}</strong>
          <Chip tone={phaseTone(card.phase)} mark testId="decision-card-status">{phaseWords(card.phase)}</Chip>
        </div>
        {open && card.deadlineAtMs !== undefined ? (
          <DeadlineChip deadlineAtMs={card.deadlineAtMs} now={now} testId="decision-card-deadline" />
        ) : null}
      </header>
      {card.statusOnly ? (
        <p className="cc-decision__note">You asked for this decision. Only its approvers see what it holds.</p>
      ) : (
        <>
          <p className="cc-approval__asked">
            {card.approverName ? (
              <span className="cc-decision__who">
                <span className="cc-decision__label">Approver</span>
                <span className="cc-decision__approver" title={card.approverId}>{card.approverName}</span>
              </span>
            ) : null}
            {card.requesterName ? (
              <span className="cc-decision__who">
                <span className="cc-decision__label">Asked by</span>
                <span className="cc-decision__requester" title={card.requester}>{card.requesterName}</span>
              </span>
            ) : null}
          </p>
          {card.rendering !== undefined ? (
            <pre className="cc-decision__rendering" data-testid="decision-card-rendering">{card.rendering}</pre>
          ) : (
            <p className="cc-decision__note">What this decision holds could not be read.</p>
          )}
          {presentations.length ? (
            <KeyValueList
              className="cc-approval__scope"
              items={presentations.map((presentation, index) => ({
                key: `${presentation.channelRef}:${presentation.messageRef}:${index}`,
                label: "Presented in",
                value: `${presentation.channelRef}, ${formatAbsoluteTime(presentation.atMs)}`,
              }))}
            />
          ) : null}
          {open ? <p className="cc-approval__hint cc-decision__hint">Decide where it was presented.</p> : null}
        </>
      )}
      <TechnicalDetails
        className="cc-approval__details"
        items={[
          { label: "Decision ID", value: card.decisionId },
          { label: "Kind", value: card.kind, plain: true },
          // Plain text: the card holds no control named for approving.
          card.statusOnly ? null : { label: "Approver ID", value: card.approverId, plain: true },
          card.statusOnly ? null : { label: "Requester", value: card.requester },
          card.decidedAtMs !== undefined ? { label: "Decided", value: formatAbsoluteTime(card.decidedAtMs), plain: true } : null,
          card.endedAtMs !== undefined ? { label: "Ended", value: formatAbsoluteTime(card.endedAtMs), plain: true } : null,
        ]}
      />
    </article>
  );
}

/** The durable decisions of the Pending tab, soonest deadline first. No card
 *  offers a decision, whether or not a person is signed in. While the last
 *  page read names a cursor, "Load more" asks for the page after it; no
 *  page is read by itself. */
export function DecisionCardList({ cards, signedIn = false, nextAfter, onLoadMore, loadingMore = false }: {
  cards: readonly DecisionCardView[];
  signedIn?: boolean;
  /** The cursor of the last page read, while more may follow. */
  nextAfter?: string;
  /** Read the page after `after`. */
  onLoadMore?: (after: string) => void;
  /** Whether that page is being read. */
  loadingMore?: boolean;
}): React.JSX.Element | null {
  const deadlines = cards.map((card) => (card.phase === "open" ? card.deadlineAtMs : undefined));
  const now = useApprovalClock(deadlines);
  const more = nextAfter !== undefined && onLoadMore !== undefined ? { after: nextAfter, load: onLoadMore } : null;
  if (!cards.length && !more) return null;
  return (
    <section
      className="gating-inbox__cards gating-inbox__decisions"
      aria-label="Decisions"
      data-signed-in={signedIn ? "true" : "false"}
      data-testid="decision-card-list"
    >
      {cards.map((card) => <DecisionCard key={card.decisionId} card={card} now={now} />)}
      {more ? (
        <button
          type="button"
          className="cc-decision__more"
          data-testid="decision-card-load-more"
          disabled={loadingMore}
          onClick={() => more.load(more.after)}
        >
          {loadingMore ? "Loading more decisions" : "Load more decisions"}
        </button>
      ) : null}
    </section>
  );
}
