import React, { useState } from "react";

import type {
  ConversationCouncilArtifactClaimRow,
  ConversationCouncilEntry,
  ConversationCouncilExchangeRow,
  ConversationCouncilParticipantRow,
  CouncilCardStatus,
} from "@console-core";

import type { IconRenderer } from "../shared";

const CARD_STATUS_LABEL: Record<CouncilCardStatus, string> = {
  completed: "Concluded",
  bounded: "Stopped at budget",
  failed: "Failed",
  pending: "Unsettled",
};

const EXCHANGE_STATUS_LABEL: Record<string, string> = {
  pending: "No terminal observed",
  completed: "Done",
  failed: "Failed",
};

// Card collapse survives remounts the same way the workgraph card's does:
// dock panels and transcript virtualisation tear the subtree down, and a
// module-level registry keyed by the stable entry id carries the flag across.
// Only `true` is stored, so the set stays small and self-pruning.
const collapsedCouncilCards = new Set<string>();

function rememberFlag(registry: Set<string>, key: string, value: boolean): void {
  if (value) registry.add(key);
  else registry.delete(key);
}

export const __councilCardUiState = {
  reset(): void {
    collapsedCouncilCards.clear();
  },
};

function exchangeStatusLabel(status: string): string {
  return EXCHANGE_STATUS_LABEL[status] || status.replace(/_/g, " ");
}

/// The typed `exit_reason` tag, humanised. The verbatim tag stays in a
/// `data-` attribute and in the title so an operator can still grep for the
/// exact variant meerkat emitted.
function exitReasonLabel(reason: string): string {
  return reason.replace(/_/g, " ");
}

function ParticipantRow({ row }: { row: ConversationCouncilParticipantRow }) {
  return (
    <li
      className={`cc-council__participant${row.seated ? "" : " is-unseated"}`}
      data-participant-order={row.order}
      data-seated={row.seated ? "true" : "false"}
    >
      <span className="cc-council__participant-role">{row.role}</span>
      <span className="cc-council__participant-identity">{row.targetIdentity}</span>
      <span className="cc-council__participant-source">from {row.sourceIdentity}</span>
      {row.seated ? null : (
        // An unseated slot is the usual cause of participant_seating_failed;
        // it is stated rather than left as an absence for the reader to spot.
        <span className="cc-council__participant-unseated">never seated</span>
      )}
    </li>
  );
}

function ExchangeRow({ row }: { row: ConversationCouncilExchangeRow }) {
  return (
    <li
      className={`cc-council__exchange is-${row.status}`}
      data-exchange-status={row.status}
      data-round={row.round}
      data-sequence={row.sequence}
    >
      <span className="cc-council__exchange-round">r{row.round + 1}</span>
      <span className="cc-council__exchange-identity">{row.targetIdentity}</span>
      <span className={`cc-council__exchange-status is-${row.status}`}>
        {exchangeStatusLabel(row.status)}
      </span>
      {row.text ? <span className="cc-council__exchange-text">{row.text}</span> : null}
      {row.truncated ? (
        <span className="cc-council__exchange-truncated" title="Truncated by the receiver bound">
          truncated
        </span>
      ) : null}
    </li>
  );
}

/// Artifact CLAIMS, never artifacts.
///
/// The council resolves nothing: no store lookup, no fetch, no existence
/// check. So this renders the uri as inert text and never as a link - a
/// clickable affordance would assert reachability nobody verified, and an
/// operator would reasonably read a dead link as a broken artifact rather
/// than as an unverified claim.
function ArtifactClaimRow({ row }: { row: ConversationCouncilArtifactClaimRow }) {
  return (
    <li className="cc-council__claim" data-claim-uri={row.uri}>
      <span className="cc-council__claim-badge">claimed</span>
      <span className="cc-council__claim-uri">{row.uri}</span>
      {row.mediaType ? <span className="cc-council__claim-media">{row.mediaType}</span> : null}
      {row.digest ? <span className="cc-council__claim-digest">{row.digest.slice(0, 12)}</span> : null}
    </li>
  );
}

/// One concluded council, rendered inline in the conversation.
///
/// Observational by construction: there are no action callbacks. Council
/// participants are forked contexts destroyed before the tool returns, so
/// there is nothing left to address by the time this renders. The only
/// interaction is collapse/expand.
export function CouncilCard({
  entry,
  Icon,
}: {
  entry: ConversationCouncilEntry;
  Icon?: IconRenderer | null;
}) {
  const [collapsed, setCollapsedState] = useState(() => collapsedCouncilCards.has(entry.id));
  const setCollapsed = (update: (value: boolean) => boolean) => {
    setCollapsedState((value) => {
      const next = update(value);
      rememberFlag(collapsedCouncilCards, entry.id, next);
      return next;
    });
  };

  const seated = entry.participants.filter((row) => row.seated).length;
  const claims = entry.artifactClaims || [];
  const debts = entry.cleanupDebts || [];
  const hasBody = entry.participants.length > 0
    || entry.exchanges.length > 0
    || claims.length > 0
    || Boolean(entry.mergeText);

  return (
    <section
      className={`cc-council is-${entry.status}${collapsed ? " is-collapsed" : ""}`}
      data-council-card=""
      data-council-id={entry.councilId}
      data-status={entry.status}
      data-exit-reason={entry.exitReason}
      data-testid={`council-card:${entry.councilId}`}
    >
      <header className="cc-council__header">
        <span className="cc-council__mark" aria-hidden="true">
          {Icon ? <Icon name="i-branch" /> : "◎"}
        </span>
        <div className="cc-council__heading">
          <span className="cc-council__title">{entry.topic}</span>
          <span className="cc-council__meta">
            {entry.participants.length} participant{entry.participants.length === 1 ? "" : "s"}
            {" · "}
            {entry.roundsCompleted} round{entry.roundsCompleted === 1 ? "" : "s"}
            {" · "}
            {entry.exchanges.length + (entry.exchangeOverflowCount || 0)} exchange
            {entry.exchanges.length + (entry.exchangeOverflowCount || 0) === 1 ? "" : "s"}
          </span>
        </div>
        <span
          className={`cc-council__badge is-${entry.status}`}
          title={`exit_reason: ${entry.exitReason}`}
        >
          {CARD_STATUS_LABEL[entry.status]}
        </span>
        {entry.replayed ? (
          <span
            className="cc-council__replayed"
            title="Answered from an existing sealed result; no new council ran"
          >
            replayed
          </span>
        ) : null}
        {hasBody ? (
          <button
            type="button"
            className="cc-council__toggle"
            aria-expanded={!collapsed}
            onClick={() => setCollapsed((value) => !value)}
          >
            {collapsed ? "Show" : "Hide"}
          </button>
        ) : null}
      </header>

      {/* A failure states its typed reason and detail up front, above the
          body, and stays visible when the card is collapsed. Burying why a
          council failed behind an expander is how a failed council gets
          mistaken for a quiet one. */}
      {entry.status === "failed" || entry.status === "pending" ? (
        <p className="cc-council__failure" role="note">
          <span className="cc-council__failure-reason">{exitReasonLabel(entry.exitReason)}</span>
          {entry.exitDetail ? (
            <span className="cc-council__failure-detail">{entry.exitDetail}</span>
          ) : null}
        </p>
      ) : null}

      {/* Cleanup debt is reported separately from status on purpose: a
          council can seal a perfectly good result and still fail to destroy
          its temporary mob. Folding that into `failed` would misreport the
          answer; hiding it would lose the obligation. */}
      {debts.length > 0 || entry.cleanupBudgetExhausted ? (
        <p className="cc-council__cleanup" role="note">
          <span className="cc-council__cleanup-label">cleanup outstanding</span>
          {entry.cleanupBudgetExhausted ? (
            <span className="cc-council__cleanup-budget">budget exhausted</span>
          ) : null}
          {debts.map((debt) => (
            <span className="cc-council__cleanup-debt" key={`${debt.subject}:${debt.detail}`}>
              {debt.subject}: {debt.detail}
            </span>
          ))}
        </p>
      ) : null}

      {collapsed || !hasBody ? null : (
        <div className="cc-council__body">
          {entry.mergeText ? (
            <div className="cc-council__merge" data-merge-kind={entry.mergeKind || ""}>
              <span className="cc-council__merge-label">
                {entry.mergeFinalizer ? `Summary by ${entry.mergeFinalizer}` : "Summary"}
              </span>
              <p className="cc-council__merge-text">{entry.mergeText}</p>
              {entry.mergeTruncated ? (
                <span className="cc-council__merge-truncated">truncated</span>
              ) : null}
            </div>
          ) : null}

          {entry.mergeKind === "no_merge" && !entry.mergeText ? (
            <p className="cc-council__no-merge">
              Observation only: the merge policy returned provenance and confirmation, no content.
            </p>
          ) : null}

          {entry.participants.length > 0 ? (
            <div className="cc-council__section">
              <span className="cc-council__section-label">
                Participants ({seated}/{entry.participants.length} seated)
              </span>
              <ul className="cc-council__participants">
                {entry.participants.map((row) => (
                  <ParticipantRow key={row.order} row={row} />
                ))}
              </ul>
            </div>
          ) : null}

          {entry.exchanges.length > 0 ? (
            <div className="cc-council__section">
              <span className="cc-council__section-label">Exchanges</span>
              <ul className="cc-council__exchanges">
                {entry.exchanges.map((row) => (
                  <ExchangeRow key={`${row.round}:${row.sequence}`} row={row} />
                ))}
              </ul>
              {entry.exchangeOverflowCount ? (
                <span className="cc-council__overflow">
                  +{entry.exchangeOverflowCount} more exchange
                  {entry.exchangeOverflowCount === 1 ? "" : "s"}
                </span>
              ) : null}
            </div>
          ) : null}

          {claims.length > 0 ? (
            <div className="cc-council__section">
              <span className="cc-council__section-label">
                Artifact claims (reported by participants, not verified)
              </span>
              <ul className="cc-council__claims">
                {claims.map((row) => (
                  <ArtifactClaimRow key={row.uri} row={row} />
                ))}
              </ul>
            </div>
          ) : null}

          <div className="cc-council__footer">
            {entry.durability ? (
              <span
                className="cc-council__durability"
                title={
                  entry.durability === "process_bound"
                    ? "Process-bound: this council does not survive a gateway restart"
                    : "Durable: recorded in the realm orchestration store"
                }
              >
                {entry.durability.replace(/_/g, " ")}
              </span>
            ) : null}
            {entry.truncatedExchangeCount ? (
              <span className="cc-council__truncated-count">
                {entry.truncatedExchangeCount} truncated
              </span>
            ) : null}
            <span className="cc-council__id">{entry.councilId}</span>
          </div>
        </div>
      )}
    </section>
  );
}
