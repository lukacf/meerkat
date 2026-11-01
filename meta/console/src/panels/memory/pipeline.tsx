// Pipeline (L1): how memory moves, as a stage flow (new memory through the
// dream into active memory; quarantine to released or discarded; waiting
// for approval to team memory), then the queues behind each stage and the
// live memory events.

import React from "react";

import { memoryAuthorWords, parseMemoryAuthor } from "../../lib/memory-event-words";
import type {
  ConsoleFrame,
  MemoryPanelRecord,
  MemoryPendingPromotion,
  MemoryProposalEntry,
} from "../../types";
import {
  AgentName,
  Button,
  Chip,
  EmptyState,
  ListRow,
  Notice,
  RelativeTime,
  RowList,
  SectionHeader,
  TechnicalId,
  cx,
  formatAbsoluteTime,
  humanizeEnum,
  joinMeta,
  plural,
} from "./kit";
import { MemoryLiveStrip } from "./live-strip";
import { realmOfRecord, type DedupedAuditVerdict } from "./model";
import { RecordFacts, RecordLink, ScopeTarget, useNameOf } from "./parts";
import { grantTitle, kindLabel, needsGrantText, reviewVerdictLabel } from "./words";

/// The stage marks, in the brand's item grammar: hollow squares wait, ink
/// squares hold, a warning outline with a slash is quarantined (a static
/// state wears its tone, never the accent).
type StageMark = "proposed" | "dream" | "active" | "quarantined" | "released" | "discarded" | "waiting" | "team";

interface StageModel {
  key: string;
  mark: StageMark;
  label: string;
  /** The count, or "no access". */
  value: number | "no access" | null;
  note?: string;
  onOpen?: () => void;
}

/// Up to eight small squares for a count; the figure carries the number.
function StageSquares({ mark, count }: { mark: StageMark; count: number }): React.JSX.Element {
  const shown = Math.min(8, Math.max(0, count));
  return (
    <span className="memory-flow__squares" aria-hidden="true">
      {Array.from({ length: shown }, (_, index) => (
        <span key={index} className="memory-flow__square" data-mark={mark} />
      ))}
      {count > shown ? <span className="memory-flow__more">{`+${count - shown}`}</span> : null}
    </span>
  );
}

function Stage({ stage }: { stage: StageModel }): React.JSX.Element {
  const count = typeof stage.value === "number" ? stage.value : 0;
  const body = (
    <>
      <span className="memory-flow__label">{stage.label}</span>{" "}
      <span className="memory-flow__count">{stage.value === null ? "" : stage.value}</span>
      {typeof stage.value === "number" && stage.mark !== "dream" ? <StageSquares mark={stage.mark} count={count} /> : null}
      {stage.note ? <span className="memory-flow__note">{stage.note}</span> : null}
    </>
  );
  return (
    <li className={cx("memory-flow__stage", count > 0 && stage.mark === "quarantined" && "is-attention")} data-stage={stage.key} data-mark={stage.mark}>
      <span className="memory-flow__mark" data-mark={stage.mark} aria-hidden="true" />
      {stage.onOpen ? (
        <button type="button" className="memory-flow__hit" onClick={stage.onOpen}>
          {body}
        </button>
      ) : (
        <span className="memory-flow__hit">{body}</span>
      )}
    </li>
  );
}

function Arrow({ dashed }: { dashed?: boolean }): React.JSX.Element {
  return <span className={cx("memory-flow__arrow", dashed && "is-dashed")} aria-hidden="true" />;
}

export function PipelineFlow({
  lanes,
}: {
  lanes: Array<{ key: string; label: string; stages: StageModel[]; split?: boolean }>;
}): React.JSX.Element {
  return (
    <figure className="memory-flow" data-testid="memory-pipeline-flow" aria-label="How memory moves">
      <ol className="memory-flow__stages" data-testid="memory-pipeline-stages" aria-label="Pipeline stages">
        {lanes.map((lane) => (
          <li className="memory-flow__lane" key={lane.key} data-lane={lane.key}>
            <span className="memory-flow__lane-label">{lane.label}</span>
            <ol className="memory-flow__lane-stages">
              {lane.stages.map((stage, index) => (
                <React.Fragment key={stage.key}>
                  {index > 0 ? (
                    lane.split && index === 2 ? (
                      <li className="memory-flow__or" aria-hidden="true">
                        or
                      </li>
                    ) : (
                      <li className="memory-flow__link" aria-hidden="true">
                        <Arrow />
                      </li>
                    )
                  ) : null}
                  <Stage stage={stage} />
                </React.Fragment>
              ))}
            </ol>
          </li>
        ))}
      </ol>
    </figure>
  );
}

/// A proposal's author in words: the agent by name, or the memory steward,
/// an operator, the application. The server's Debug text never shows.
function ProposalAuthor({ value }: { value: string | undefined }): React.JSX.Element | null {
  const nameOf = useNameOf();
  const author = parseMemoryAuthor(value);
  if (!author) return null;
  return (
    <>
      {" · proposed by "}
      {author.author === "agent" ? <AgentName id={author.identity} link /> : memoryAuthorWords(author, { agentName: nameOf })}
    </>
  );
}

export function PipelineTab({
  flow,
  proposals,
  proposalsDenied,
  pendingPromotions,
  quarantineRecords,
  canReviewQuarantine,
  canDecideQuarantine,
  auditVerdicts,
  auditVerdictsDenied,
  recordTitles,
  memoryFrames,
  onSelectRecord,
  onOpenGating,
}: {
  flow: Array<{ key: string; label: string; stages: StageModel[]; split?: boolean }>;
  proposals: MemoryProposalEntry[];
  proposalsDenied: boolean;
  pendingPromotions: MemoryPendingPromotion[];
  quarantineRecords: MemoryPanelRecord[];
  canReviewQuarantine: boolean;
  canDecideQuarantine: boolean;
  auditVerdicts: DedupedAuditVerdict[];
  auditVerdictsDenied: boolean;
  recordTitles: ReadonlyMap<string, string>;
  memoryFrames: ConsoleFrame[];
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
  onOpenGating?: () => void;
}): React.JSX.Element {
  return (
    <div className="memory-tab memory-pipeline" data-testid="memory-pipeline">
      <PipelineFlow lanes={flow} />
      <Notice testId="memory-quarantine-note">
        {canDecideQuarantine
          ? "Open a quarantined record to release or discard it. Records waiting to move to team memory are decided in Approvals; the memory steward's dream decides the rest."
          : "Read only. Decisions come from the memory steward's dream, from Approvals, and from reviewers who can review quarantine and write memory."}
      </Notice>

      <section className="memory-block" data-testid="memory-pipeline-proposals" aria-labelledby="memory-proposed-head">
        <SectionHeader
          id="memory-proposed-head"
          label="Proposed"
          count={!proposalsDenied && proposals.length > 0 ? proposals.length : null}
        />
        {proposalsDenied ? (
          <EmptyState inline kind="no-access" title="Proposals: no access." />
        ) : proposals.length === 0 ? (
          <EmptyState inline title="No proposal is waiting for a dream." />
        ) : (
          <RowList>
            {proposals.map((proposal) => (
              <ListRow
                key={`${proposal.realm}:${proposal.proposal_id}`}
                className="memory-proposal-row"
                testId={`memory-proposal:${proposal.proposal_id}`}
                lead={<span className="memory-flow__square" data-mark="proposed" aria-hidden="true" />}
                title={proposal.title || "Untitled proposal"}
                titleAttr={joinMeta(proposal.title, `proposal ${proposal.proposal_id}`)}
                subtitle={
                  <>
                    {`${kindLabel(proposal.kind)} for `}
                    <ScopeTarget kind={proposal.scope_kind} scopeKey={proposal.scope_key} />
                    <ProposalAuthor value={proposal.author} />
                  </>
                }
                meta={
                  <>
                    {proposal.tainted ? (
                      <Chip
                        tone="warn"
                        testId={`memory-proposal-taint:${proposal.proposal_id}`}
                        title="Tainted: proposed in a session that read untrusted input."
                      >
                        Untrusted source
                      </Chip>
                    ) : null}
                    {proposal.status && proposal.status !== "pending" ? (
                      <Chip tone={proposal.status === "held" ? "warn" : "neutral"}>{humanizeEnum(proposal.status)}</Chip>
                    ) : null}
                    <RelativeTime value={proposal.created_at_ms} />
                  </>
                }
              />
            ))}
          </RowList>
        )}
      </section>

      {canReviewQuarantine ? (
        <>
          <section className="memory-block" aria-labelledby="memory-waiting-head">
            <SectionHeader
              id="memory-waiting-head"
              label="Waiting for approval"
              count={pendingPromotions.length > 0 ? pendingPromotions.length : null}
            />
            {pendingPromotions.length === 0 ? (
              <EmptyState inline title="No record is waiting to move." />
            ) : (
              <RowList>
                {pendingPromotions.map((pending) => (
                  <ListRow
                    key={pending.pending_id}
                    className="memory-pending-row"
                    testId={`memory-pending:${pending.pending_id}`}
                    onActivate={() => onSelectRecord(pending.realm, pending.record_id)}
                    lead={<span className="memory-flow__square" data-mark="waiting" aria-hidden="true" />}
                    titleAttr={`Promotion ${pending.pending_id}`}
                    title={recordTitles.get(pending.record_id) ?? <TechnicalId value={pending.record_id} copy={false} />}
                    subtitle={
                      <>
                        {"To "}
                        <ScopeTarget kind={pending.scope_kind} scopeKey={pending.scope_key} />
                      </>
                    }
                    meta={
                      <>
                        {pending.status && pending.status !== "pending" ? <Chip>{humanizeEnum(pending.status)}</Chip> : null}
                        <RelativeTime value={pending.created_at_ms} />
                      </>
                    }
                    actions={
                      onOpenGating ? (
                        <Button size="sm" data-testid={`memory-pipeline-decide:${pending.pending_id}`} onClick={onOpenGating}>
                          Decide in Approvals
                        </Button>
                      ) : undefined
                    }
                  >
                    {pending.rationale ? (
                      <span className="memory-row-note" title={pending.rationale}>
                        {pending.rationale}
                      </span>
                    ) : null}
                  </ListRow>
                ))}
              </RowList>
            )}
          </section>
          <section className="memory-block" aria-labelledby="memory-quarantined-head">
            <SectionHeader
              id="memory-quarantined-head"
              label="Quarantined"
              count={quarantineRecords.length > 0 ? quarantineRecords.length : null}
            />
            {quarantineRecords.length === 0 ? (
              <EmptyState inline title="Nothing is quarantined." />
            ) : (
              <RowList>
                {quarantineRecords.map((record) => {
                  const reason = record.status.status === "quarantined" ? record.status.reason : undefined;
                  return (
                    <ListRow
                      key={record.id}
                      className="memory-quarantine-row"
                      testId={`memory-quarantine-record:${record.id}`}
                      onActivate={() => onSelectRecord(realmOfRecord(record), record.id)}
                      lead={<span className="memory-flow__square" data-mark="quarantined" aria-hidden="true" />}
                      title={record.title || "Untitled record"}
                      titleAttr={record.title || undefined}
                      subtitle={<RecordFacts record={record} hideStatus />}
                      meta={<RelativeTime value={record.created_at_ms} />}
                    >
                      {reason ? (
                        <span className="memory-row-note" title={reason}>
                          {reason}
                        </span>
                      ) : null}
                    </ListRow>
                  );
                })}
              </RowList>
            )}
          </section>
        </>
      ) : (
        <Notice tone="locked" testId="memory-pipeline-no-grant">
          <span title={grantTitle("memory.quarantine.review")}>
            {`Quarantine: no access. ${needsGrantText("memory.quarantine.review")} to see the queue.`}
          </span>
        </Notice>
      )}

      <section className="memory-block" data-testid="memory-review-queue" aria-labelledby="memory-review-head">
        <SectionHeader
          id="memory-review-head"
          label="Worth a look"
          count={!auditVerdictsDenied && auditVerdicts.length > 0 ? auditVerdicts.length : null}
        />
        {auditVerdictsDenied ? (
          <EmptyState inline kind="no-access" title="Review queue: no access." />
        ) : auditVerdicts.length === 0 ? (
          <EmptyState inline title="Nothing is flagged for a closer look." />
        ) : (
          <>
            <p className="memory-caption">Records a dream flagged for a closer look, once each.</p>
            <RowList>
              {auditVerdicts.map(({ verdict, flags }) => (
                <ListRow
                  key={`${verdict.realm}:${verdict.run_id}:${verdict.record_id}`}
                  className="memory-review-row"
                  testId={`memory-review:${verdict.run_id}:${verdict.record_id}`}
                  lead={<span className="memory-review-mark" data-tone="warn" aria-hidden="true" />}
                  title={
                    <RecordLink
                      id={verdict.record_id}
                      realm={verdict.realm}
                      titles={recordTitles}
                      testId={`memory-review-record:${verdict.run_id}:${verdict.record_id}`}
                      onOpen={onSelectRecord}
                    />
                  }
                  subtitle={verdict.rationale}
                  meta={
                    <>
                      {verdict.verdict ? <Chip tone="warn">{reviewVerdictLabel(verdict.verdict)}</Chip> : null}
                      {flags > 1 ? <span className="memory-review-row__flags">{`flagged ${plural(flags, "time")}`}</span> : null}
                      <RelativeTime
                        value={verdict.created_at_ms}
                        prefix="Flagged"
                        title={joinMeta(formatAbsoluteTime(verdict.created_at_ms), `dream run ${verdict.run_id}`)}
                      />
                    </>
                  }
                />
              ))}
            </RowList>
          </>
        )}
      </section>

      <MemoryLiveStrip
        frames={memoryFrames}
        recordTitles={recordTitles}
        onPivot={(realm, recordId) => void onSelectRecord(realm, recordId)}
      />
    </div>
  );
}

export type { StageModel };
