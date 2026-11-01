// A record's biography (L2, the drawer): what it says, where it came from,
// how it changed, how agents used it, and who reviewed it, as one vertical
// story (BORN, VERSIONS, USE, REVIEWS), then its technical details.

import React from "react";

import type { ConversationTimelineEntry } from "@console-core";
import { CopyButton } from "../../../../packages/console-components/src/copy-button";
import type {
  MemoryAuditVerdictEntry,
  MemoryDreamRun,
  MemoryEvidenceRef,
  MemoryFullRecord,
  MemoryQuarantineVerdict,
} from "../../types";
import {
  AgentName,
  Button,
  Chip,
  DetailDrawer,
  type TechnicalDetail,
  KeyValueList,
  ListRow,
  Notice,
  RelativeTime,
  RowList,
  StatusPill,
  TechnicalId,
  TrustLadder,
  formatAbsoluteTime,
  plural,
  useNow,
} from "./kit";
import {
  dreamRunsTouching,
  evidenceExcerptLines,
  lineageLane,
  realmOfRecord,
  type MemoryRecordDetail,
} from "./model";
import {
  FluidSparkline,
  KindMark,
  QuarantineNoticeText,
  statusMarkTone,
  useNameOf,
} from "./parts";
import {
  QUARANTINE_RATIONALE_MAX_BYTES,
  canDecideQuarantinedRecord,
  quarantineDecisionErrorText,
  quarantineRationaleProblem,
  type QuarantineNotice,
} from "./quarantine";
import { VersionsGraph, versionsModel } from "./versions";
import { DAY_MS, binCounts, trustLevel } from "./viz-model";
import {
  authorDisplay,
  evidenceDetails,
  evidenceDisplay,
  injectionMoment,
  kindLabel,
  reviewVerdictLabel,
  scopeHeading,
  scopeKeyOf,
  statusTitle,
  statusWord,
  trustDisplay,
} from "./words";

interface EvidenceState {
  key: string;
  /// "not-found": the session did not appear in the recent timeline window
  /// (it may merely be older than the window; never claim it is gone).
  /// "empty-range": the session was found but the evidence range holds no
  /// renderable message entries.
  status: "loading" | "loaded" | "not-found" | "empty-range";
  lines: ReturnType<typeof evidenceExcerptLines>;
}

function evidenceKey(evidence: MemoryEvidenceRef, index: number): string {
  return `${index}:${evidence.session_id || ""}:${evidence.generation ?? ""}`;
}

/// How many days the USE bars cover.
const USE_DAYS = 14;
/// The ledger rows a record's read carries at most (the server's
/// MEMORY_PANEL_INJECTIONS_MAX): at the cap, the count reads "50+".
const INJECTION_ROWS_MAX = 50;

/// The decision block on a quarantined record's Biography. The body and
/// provenance above it are what the reviewer decides on; the request binds
/// the record's content hash so a changed record is refused, not decided.
function QuarantineDecisionBlock({
  record,
  onDecide,
}: {
  record: MemoryFullRecord;
  onDecide: (verdict: MemoryQuarantineVerdict, rationale: string | undefined) => Promise<void>;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const rationaleId = React.useId();
  const [rationale, setRationale] = React.useState("");
  const [pending, setPending] = React.useState<MemoryQuarantineVerdict | null>(null);
  const [failure, setFailure] = React.useState<QuarantineNotice | null>(null);
  const rationaleProblem = quarantineRationaleProblem(rationale);
  const blocked = pending !== null || rationaleProblem !== null;
  const notice: QuarantineNotice | null = rationaleProblem ? { text: rationaleProblem, ids: [] } : failure;
  const owner =
    record.scope.scope === "identity"
      ? `${nameOf(record.scope.identity) || record.scope.identity}'s memory`
      : "this memory";
  const reason = record.status.status === "quarantined" ? record.status.reason : undefined;
  async function decide(verdict: MemoryQuarantineVerdict): Promise<void> {
    if (rationaleProblem !== null) return;
    setPending(verdict);
    setFailure(null);
    try {
      await onDecide(verdict, rationale.trim() || undefined);
    } catch (err) {
      setFailure(quarantineDecisionErrorText(err));
    } finally {
      setPending(null);
    }
  }
  return (
    <section className="memory-review" data-testid="memory-quarantine-decide" aria-labelledby={`${rationaleId}-head`}>
      <div className="memory-review__head">
        <StatusPill tone="warn">Quarantined</StatusPill>
        <h4 className="memory-review__title" id={`${rationaleId}-head`}>
          Needs your review
        </h4>
      </div>
      {reason ? <p className="memory-review__reason">{reason}</p> : null}
      <p className="memory-review__note">
        Release makes this an active record in {owner} at observed trust. Because it was
        quarantined, it can never reach verified trust. Discard marks it deleted for good, so
        it is never recalled.
      </p>
      <label className="memory-review__label" htmlFor={rationaleId}>
        Rationale, optional and kept in the audit log
      </label>
      <input
        id={rationaleId}
        className="cc-input memory-quarantine__rationale"
        aria-label="Rationale"
        data-testid="memory-quarantine-rationale"
        placeholder="Why you release or discard it"
        value={rationale}
        maxLength={QUARANTINE_RATIONALE_MAX_BYTES * 2}
        disabled={pending !== null}
        onChange={(event) => setRationale(event.target.value)}
      />
      <div className="cc-btn-row memory-review__actions">
        <Button data-testid="memory-quarantine-release" disabled={blocked} onClick={() => void decide("release")}>
          {pending === "release" ? "Releasing…" : "Release"}
        </Button>
        <Button
          variant="danger"
          data-testid="memory-quarantine-tombstone"
          title="Tombstone: keeps only a deletion marker; the record is never recalled."
          disabled={blocked}
          onClick={() => void decide("tombstone")}
        >
          {pending === "tombstone" ? "Discarding…" : "Discard"}
        </Button>
      </div>
      {notice ? (
        <Notice tone="crit" testId="memory-quarantine-error">
          <QuarantineNoticeText notice={notice} />
        </Notice>
      ) : null}
    </section>
  );
}

/// One step of the biography story: a mark on the spine, a micro-label and
/// its content.
function StoryStep({
  step,
  label,
  meta,
  testId,
  children,
}: {
  step: "born" | "versions" | "use" | "reviews";
  label: string;
  meta?: React.ReactNode;
  testId: string;
  children?: React.ReactNode;
}): React.JSX.Element {
  return (
    <li className="memory-story__step" data-step={step} data-testid={testId}>
      <span className="memory-story__mark" aria-hidden="true" />
      <div className="memory-story__head">
        <h4 className="memory-story__label">{label}</h4>
        {meta ? <span className="memory-story__meta">{meta}</span> : null}
      </div>
      <div className="memory-story__body">{children}</div>
    </li>
  );
}

export function Biography({
  detail,
  dreams,
  auditVerdicts,
  recordTitles,
  onClose,
  onSelectRecord,
  onLoadEvidence,
  onDecideQuarantine,
  decisionNotice,
}: {
  detail: MemoryRecordDetail;
  dreams: MemoryDreamRun[];
  auditVerdicts: MemoryAuditVerdictEntry[];
  recordTitles: ReadonlyMap<string, string>;
  onClose: () => void;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
  onLoadEvidence?: (identity: string | undefined, evidence: MemoryEvidenceRef) => Promise<ConversationTimelineEntry[] | null>;
  onDecideQuarantine?: (
    record: MemoryFullRecord,
    verdict: MemoryQuarantineVerdict,
    rationale: string | undefined,
  ) => Promise<void>;
  decisionNotice?: QuarantineNotice | null;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const now = useNow();
  const { record, chain, injections } = detail;
  const provenance = record.provenance;
  const author = provenance?.author;
  const evidence = provenance?.evidence || [];
  const verification = provenance?.verification;
  const usage = record.usage;
  const lane = lineageLane(chain, record.id);
  const touchingRuns = dreamRunsTouching(dreams, record.id);
  const verdicts = auditVerdicts.filter((verdict) => verdict.record_id === record.id);
  const [evidenceState, setEvidenceState] = React.useState<EvidenceState | null>(null);
  // Monotonic click sequence: a slow earlier evidence fetch must never
  // overwrite the excerpt of a later click (same discipline as the records
  // pager). Lives in a ref because openEvidence is re-created per render.
  const evidenceSeqRef = React.useRef(0);
  const recordIdentity =
    record.scope.scope === "identity"
      ? record.scope.identity
      : author?.author === "agent"
        ? author.identity
        : undefined;
  const conversationWith = recordIdentity ? nameOf(recordIdentity) || recordIdentity : undefined;
  const supersededBy = record.status?.status === "superseded" ? record.status.by : undefined;
  const authorRun =
    author && (author.author === "steward" || author.author === "distiller") ? author.run_id : undefined;
  const scopeKey = scopeKeyOf(record.scope);
  const versions = React.useMemo(
    () => versionsModel(lane.map((entry) => entry.record), record.id, record.derived_from ?? [], recordTitles, now),
    [lane, record.id, record.derived_from, recordTitles, now],
  );
  const useBins = React.useMemo(
    () => binCounts(injections.map((injection) => injection.at_ms), now, USE_DAYS, DAY_MS),
    [injections, now],
  );
  const givenRecently = useBins.reduce((sum, count) => sum + count, 0);
  // USE counts what it lists: the ledger rows the server sends with the
  // record (its newest INJECTION_ROWS_MAX), never a separate counter that
  // can disagree with them. Recalls and useful verdicts have no rows here,
  // so they stay the record's own counters.
  const givenText = injections.length >= INJECTION_ROWS_MAX ? `${INJECTION_ROWS_MAX}+` : String(injections.length);
  const lastGivenAt = injections.reduce((latest, injection) => Math.max(latest, injection.at_ms || 0), 0) || undefined;

  async function openEvidence(ref: MemoryEvidenceRef, index: number): Promise<void> {
    if (!onLoadEvidence) return;
    const key = evidenceKey(ref, index);
    const seq = ++evidenceSeqRef.current;
    setEvidenceState({ key, status: "loading", lines: [] });
    const entries = await onLoadEvidence(recordIdentity, ref);
    if (seq !== evidenceSeqRef.current) return;
    const lines = entries ? evidenceExcerptLines(entries, ref.range) : [];
    setEvidenceState({
      key,
      status: entries === null ? "not-found" : lines.length > 0 ? "loaded" : "empty-range",
      lines,
    });
  }

  const technical: Array<TechnicalDetail | null> = [
          { label: "Memory ID", value: record.id },
          { label: "Scope", value: record.scope.scope, plain: true },
          scopeKey ? { label: "Scope key", value: scopeKey } : null,
          { label: "Realm", value: detail.realm, plain: true },
          { label: "Kind", value: record.kind, plain: true },
          { label: "Trust", value: record.trust, plain: true },
          { label: "Content hash", value: record.content_hash },
          { label: "Replaces", value: record.supersedes },
          { label: "Replaced by", value: supersededBy },
          record.created_at_ms ? { label: "Written", value: formatAbsoluteTime(record.created_at_ms), plain: true } : null,
          record.derived_from && record.derived_from.length > 0
            ? {
                label: "Derived from",
                value: (
                  <span className="memory-tech-list">
                    {record.derived_from.map((id) => (
                      <TechnicalId key={id} value={id} label="Record ID" />
                    ))}
                  </span>
                ),
              }
            : null,
          { label: "Author run", value: authorRun },
          // Each evidence reference's session, generation and revision: the
          // evidence link above shows only the conversation and its messages.
          ...evidence.map((ref, index) => {
            const details = evidenceDetails(ref);
            return ref.session_id || details
              ? {
                  label: evidence.length > 1 ? `Evidence ${index + 1}` : "Evidence",
                  value: (
                    <span className="memory-tech-list">
                      {ref.session_id ? <TechnicalId value={ref.session_id} label="Session ID" /> : null}
                      {details ? <span className="memory-tech-note">{details}</span> : null}
                    </span>
                  ),
                }
              : null;
          }),
          touchingRuns.length > 0 || verdicts.length > 0
            ? {
                label: "Dream runs",
                value: (
                  <span className="memory-tech-list">
                    {[...new Set([...verdicts.map((verdict) => verdict.run_id), ...touchingRuns.map((run) => run.run_id)])].map((runId) => (
                      <TechnicalId key={runId} value={runId} label="Dream run ID" />
                    ))}
                  </span>
                ),
              }
            : null,
  ];

  const owner = (
    <>
      {record.scope.scope === "identity" ? (
        <>
          <AgentName id={record.scope.identity} link />
          {"'s memory"}
        </>
      ) : (
        scopeHeading(record.scope, nameOf)
      )}
      {record.updated_at_ms ? (
        <>
          {", updated "}
          <RelativeTime value={record.updated_at_ms} />
        </>
      ) : null}
    </>
  );

  return (
    <DetailDrawer
      className="memory-detail"
      testId="memory-detail"
      closeTestId="memory-detail-back"
      onClose={onClose}
      ariaLabel={`Record: ${record.title || "Untitled record"}`}
      mark={<KindMark kind={record.kind} className="memory-detail__kind" />}
      title={record.title || "Untitled record"}
      subtitle={
        <span className="memory-detail__facts">
          <StatusPill tone={statusMarkTone(record.status)} title={statusTitle(record.status)}>
            {statusWord(record.status)}
          </StatusPill>
          <span className="memory-facts__kind">{kindLabel(record.kind)}</span>
          <TrustLadder level={trustLevel(record.trust)} label={trustDisplay(record.trust)} />
          <span className="memory-detail__owner">{owner}</span>
        </span>
      }
      actions={<CopyButton text={JSON.stringify(record, null, 2)} label="Copy record JSON" className="memory-copy-json" />}
      technical={technical}
    >
      {supersededBy ? (
        <Notice testId="memory-detail-replaced" className="memory-detail__notice">
          A newer version replaced this record.{" "}
          <button type="button" className="memory-link" onClick={() => onSelectRecord(realmOfRecord(record), supersededBy)}>
            Open the newer version
          </button>
        </Notice>
      ) : null}

      {decisionNotice ? (
        <Notice tone="ok" testId="memory-quarantine-outcome" className="memory-detail__notice">
          <QuarantineNoticeText notice={decisionNotice} />
        </Notice>
      ) : null}

      {onDecideQuarantine && canDecideQuarantinedRecord(record) ? (
        <QuarantineDecisionBlock
          key={record.id}
          record={record}
          onDecide={(verdict, rationale) => onDecideQuarantine(record, verdict, rationale)}
        />
      ) : null}

      {record.description && !(record.body ?? "").trim().startsWith(record.description.trim()) ? (
        <p className="memory-detail__description">{record.description}</p>
      ) : null}

      <div className="memory-detail__body" data-testid="memory-detail-body">
        {record.body}
      </div>

      {record.tags && record.tags.length > 0 ? (
        <div className="memory-detail__tags">
          {record.tags.map((tag) => (
            <span className="memory-tag" key={tag}>
              {tag}
            </span>
          ))}
        </div>
      ) : null}

      <ol className="memory-story" aria-label="Biography">
        <StoryStep
          step="born"
          label="Born"
          testId="memory-detail-born"
          meta={record.created_at_ms ? <RelativeTime value={record.created_at_ms} /> : undefined}
        >
          <KeyValueList
            className="memory-story__kv"
            items={[
              {
                key: "author",
                label: "Written by",
                value:
                  author?.author === "agent" && author.identity ? (
                    <>
                      <AgentName id={author.identity} link />
                      {" (agent)"}
                    </>
                  ) : (
                    authorDisplay(author, nameOf)
                  ),
              },
              evidence.length > 0
                ? {
                    key: "evidence",
                    label: "Evidence",
                    value: (
                      <span className="memory-evidence">
                        {evidence.map((ref, index) => (
                          // The session ID, generation and revision are in
                          // the technical details below, not on the link.
                          <button
                            type="button"
                            className="memory-evidence__ref"
                            key={`ev-${index}`}
                            data-testid={`memory-evidence:${index}`}
                            onClick={() => void openEvidence(ref, index)}
                            disabled={!onLoadEvidence}
                            title={onLoadEvidence ? "Open the conversation" : undefined}
                          >
                            <span className="memory-evidence__label">{evidenceDisplay(ref, conversationWith)}</span>
                          </button>
                        ))}
                      </span>
                    ),
                  }
                : null,
              verification?.checked ? { key: "verified", label: "Verified", value: verification.checked } : null,
              record.ever_quarantined ? { key: "quarantine", label: "Quarantine", value: "Once quarantined, then released" } : null,
            ]}
          />
          {evidenceState ? (
            evidenceState.status === "loading" ? (
              <p className="memory-caption" role="status">
                Loading the conversation…
              </p>
            ) : evidenceState.status === "not-found" ? (
              <Notice testId="memory-evidence-degraded">
                This conversation was not found in the recent timeline window. It may be older
                than the window; only its reference is kept here.
              </Notice>
            ) : evidenceState.status === "empty-range" ? (
              <Notice testId="memory-evidence-empty">
                The conversation was found, but it has no messages in this range. Ranges are
                matched to the console timeline approximately.
              </Notice>
            ) : (
              <div className="memory-excerpt" data-testid="memory-evidence-excerpt">
                <p className="memory-excerpt__note">Matched to the console timeline approximately.</p>
                {evidenceState.lines.map((line) => (
                  <div className="memory-excerpt__line" key={line.id}>
                    <span className="memory-excerpt__speaker">{line.speaker}</span>
                    <span className="memory-excerpt__text">{line.text}</span>
                  </div>
                ))}
              </div>
            )
          ) : null}
        </StoryStep>

        {lane.length > 0 ? (
          <StoryStep
            step="versions"
            label="Versions"
            testId="memory-detail-lineage"
            meta={lane.length > 1 ? plural(lane.length, "version") : "Only version"}
          >
            {versions.nodes.length > 1 ? (
              <VersionsGraph
                model={versions}
                openId={record.id}
                testId="memory-biography-versions"
                onOpen={(realm, id) => onSelectRecord(realm, id)}
              />
            ) : null}
            <RowList className="memory-chain">
              {lane.map(({ record: entry, current }, index) => (
                <ListRow
                  key={entry.id}
                  className="memory-chain__row"
                  dense
                  selected={current}
                  testId={`memory-chain:${entry.id}`}
                  dataAttributes={{
                    "data-current": current ? "true" : undefined,
                    "data-dimmed": current ? undefined : "true",
                  }}
                  onActivate={() => {
                    if (!current) onSelectRecord(realmOfRecord(entry), entry.id);
                  }}
                  lead={<span className="memory-chain__index">{`v${index + 1}`}</span>}
                  title={entry.title || "Untitled record"}
                  titleAttr={entry.title || undefined}
                  meta={
                    <>
                      <TrustLadder level={trustLevel(entry.trust)} label={trustDisplay(entry.trust)} className="memory-ladder--bare" />
                      <StatusPill tone={statusMarkTone(entry.status)} variant="quiet" title={statusTitle(entry.status)}>
                        {statusWord(entry.status)}
                      </StatusPill>
                      {current ? (
                        <span className="memory-chain__current">Open</span>
                      ) : (
                        <RelativeTime value={entry.updated_at_ms} />
                      )}
                    </>
                  }
                />
              ))}
            </RowList>
          </StoryStep>
        ) : null}

        <StoryStep
          step="use"
          label="Use"
          testId="memory-detail-life"
          meta={lastGivenAt ? <RelativeTime value={lastGivenAt} prefix="Last given" /> : undefined}
        >
          <div className="memory-use">
            <div className="memory-use__figures">
              <span className="memory-use__figure">
                <span className="memory-use__value">{givenText}</span>
                <span className="memory-use__unit">given</span>
              </span>
              <span className="memory-use__figure">
                <span className="memory-use__value">{usage?.explicit_recall_count ?? 0}</span>
                <span className="memory-use__unit">recalled</span>
              </span>
              <span className="memory-use__figure">
                <span className="memory-use__value">{usage?.judged_useful_count ?? 0}</span>
                <span className="memory-use__unit">useful</span>
              </span>
            </div>
            <div className="memory-use__chart">
              <FluidSparkline
                values={useBins}
                kind="bars"
                maxWidth={176}
                height={32}
                label={`Given to agents ${plural(givenRecently, "time")} in the last ${USE_DAYS} days`}
              />
              <span className="memory-use__axis" aria-hidden="true">
                <span>{USE_DAYS} days ago</span>
                <span>today</span>
              </span>
            </div>
          </div>
          <p className="memory-usage">
            {usage || injections.length > 0 ? (
              <>
                given to agents {givenText} · recalled {usage?.explicit_recall_count ?? 0} ·
                judged useful {usage?.judged_useful_count ?? 0}
                {lastGivenAt ? (
                  <>
                    {" · last given "}
                    <RelativeTime value={lastGivenAt} />
                  </>
                ) : null}
              </>
            ) : (
              "No usage recorded yet."
            )}
          </p>
          {injections.length > 0 ? (
            <RowList className="memory-detail__rows">
              {injections.map((injection, index) => (
                <ListRow
                  key={`inj-${index}`}
                  dense
                  testId={`memory-detail-injection:${index}`}
                  lead={<span className="memory-given" data-surface={injection.surface} aria-hidden="true" />}
                  title={
                    <>
                      {"Given to "}
                      <AgentName id={injection.identity} link />
                      {` ${injectionMoment(injection.surface)}`}
                    </>
                  }
                  titleAttr={injection.session_key ? `Session ${injection.session_key}` : undefined}
                  meta={<RelativeTime value={injection.at_ms} />}
                />
              ))}
            </RowList>
          ) : (
            <p className="memory-caption">This record has not been given to an agent yet.</p>
          )}
        </StoryStep>

        <StoryStep
          step="reviews"
          label="Reviews"
          testId="memory-detail-dreams"
          meta={touchingRuns.length + verdicts.length > 0 ? plural(touchingRuns.length + verdicts.length, "review") : undefined}
        >
          {touchingRuns.length > 0 || verdicts.length > 0 ? (
            <RowList className="memory-detail__rows">
              {verdicts.map((verdict) => (
                <ListRow
                  key={`verdict:${verdict.run_id}`}
                  dense
                  lead={<span className="memory-review-mark" data-tone="warn" aria-hidden="true" />}
                  title={verdict.verdict ? `Flagged: ${reviewVerdictLabel(verdict.verdict).toLowerCase()}` : "Flagged for a closer look"}
                  titleAttr={`Dream run ${verdict.run_id}`}
                  subtitle={verdict.rationale}
                  meta={<RelativeTime value={verdict.created_at_ms} />}
                />
              ))}
              {touchingRuns.map((run) => (
                <ListRow
                  key={run.run_id}
                  dense
                  lead={<span className="memory-review-mark" aria-hidden="true" />}
                  title="Reviewed by a dream"
                  titleAttr={`Dream run ${run.run_id}`}
                  meta={
                    <>
                      {run.quarantined_ops ? (
                        <Chip tone="warn">{`${plural(run.quarantined_ops, "change")} quarantined`}</Chip>
                      ) : null}
                      <RelativeTime value={run.last_op_at_ms || run.first_op_at_ms} />
                    </>
                  }
                />
              ))}
            </RowList>
          ) : (
            <p className="memory-caption">
              No sampled dream run mentions this record. Each dream run lists a sample of up to 12
              of the records it touched.
            </p>
          )}
        </StoryStep>
      </ol>

    </DetailDrawer>
  );
}
