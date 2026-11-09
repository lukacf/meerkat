// Holdings (L1): the six checks as one instrument strip, "Where memory
// lives" (one row per scope: its composition, a 30-day timeline of the
// records it holds and its size against the per-scope limit), what is in
// progress, and the retired agents awaiting their summary.

import React from "react";

import type {
  MemoryDreamRun,
  MemoryHarvestEntry,
  MemoryPanelRecord,
  MemoryPendingPromotion,
  MemoryProposalEntry,
  MemoryRecordScope,
  MemoryScopeOverview,
} from "../../types";
import {
  AgentAvatar,
  AgentName,
  Button,
  Chip,
  DotStrip,
  EmptyState,
  KitGlyph,
  ListRow,
  LoadingState,
  MiniBar,
  Notice,
  RelativeTime,
  RowList,
  SectionHeader,
  StatTile,
  StatusPill,
  cx,
  formatAbsoluteTime,
  formatRelativeTime,
  humanizeEnum,
  joinMeta,
  plural,
  type DotStripItem,
} from "./kit";
import {
  LATTICE_WALK_MAX_RECORDS,
  filterForOverviewScope,
  filterForScope,
  overviewScopeKey,
  scopeOverviewRows,
  verdictStatusLabel,
  verdictTone,
  type MemoryRecordsFilter,
  type ScopeOverviewRow,
  type VerdictTile,
} from "./model";
import { MetaLine, RecordLink, useMeasuredWidth, useNameOf } from "./parts";
import { DAY_MS, landscapeLanes, landscapeWindow, type LandscapeLane } from "./viz-model";
import {
  formatSize,
  grantTitle,
  needsGrantText,
  realmSpaceName,
  scopeCountsText,
  scopeKeyOf,
  scopeKindLabel,
  scopeOwnerName,
  statusWord,
} from "./words";

// ── Checks ────────────────────────────────────────────────────────────────

/// The two checks this console can never run close the strip, quieter.
const MUTED_TILES: ReadonlySet<VerdictTile["id"]> = new Set(["echo-safety", "taint-wall"]);

function CheckCell({
  tile,
  muted,
  recordTitles,
  onOpen,
  onOpenRecord,
}: {
  tile: VerdictTile;
  muted: boolean;
  recordTitles: ReadonlyMap<string, string>;
  onOpen: (tile: VerdictTile) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const tone = verdictTone(tile.status);
  const evidence = tile.evidence ?? [];
  return (
    <StatTile
      className={cx("memory-check", muted && "memory-check--quiet")}
      testId={`memory-verdict:${tile.id}`}
      dataAttributes={{ "data-status": tile.status }}
      label={tile.label}
      tone={tone}
      help={tile.lines.join("\n")}
      status={
        <>
          <StatusPill tone={tone} variant={muted ? "quiet" : "solid"} className="memory-check__pill">
            {verdictStatusLabel(tile.status)}
          </StatusPill>
          {tile.rechecking ? (
            // Over the cell's corner, outside the text flow: a re-check never
            // grows the cell or moves the cells around it.
            <span className="memory-check__recheck" data-testid={`memory-verdict-recheck:${tile.id}`}>
              <span className="memory-check__recheck-mark" aria-hidden="true" />
              re-checking
            </span>
          ) : null}
        </>
      }
      summary={tile.summary}
      onActivate={() => onOpen(tile)}
    >
      {evidence.length > 0 ? (
        <>
          {evidence.map((violation, index) => (
            <RecordLink
              key={`${violation.realm}:${violation.id}:${index}`}
              className="memory-check__evidence"
              id={violation.id}
              realm={violation.realm}
              titles={recordTitles}
              testId={`memory-verdict-evidence:${tile.id}:${violation.id}`}
              onOpen={onOpenRecord}
            />
          ))}
          {tile.evidenceMore ? <span className="memory-check__more">{`+${tile.evidenceMore} more`}</span> : null}
        </>
      ) : null}
    </StatTile>
  );
}

export function ChecksStrip({
  tiles,
  recordTitles,
  onOpen,
  onOpenRecord,
}: {
  tiles: VerdictTile[];
  recordTitles: ReadonlyMap<string, string>;
  onOpen: (tile: VerdictTile) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const main = tiles.filter((tile) => !MUTED_TILES.has(tile.id));
  const quiet = tiles.filter((tile) => MUTED_TILES.has(tile.id));
  const holding = tiles.filter((tile) => tile.status === "holding").length;
  return (
    <section className="memory-block memory-checks-block" aria-labelledby="memory-checks-head">
      <SectionHeader
        id="memory-checks-head"
        label="Checks"
        action={<span className="memory-block__meta">{`${holding} of ${tiles.length} holding`}</span>}
      />
      <div className="memory-checks" data-testid="memory-verdict-strip">
        {main.map((tile) => (
          <CheckCell key={tile.id} tile={tile} muted={false} recordTitles={recordTitles} onOpen={onOpen} onOpenRecord={onOpenRecord} />
        ))}
        {quiet.map((tile) => (
          <CheckCell key={tile.id} tile={tile} muted recordTitles={recordTitles} onOpen={onOpen} onOpenRecord={onOpenRecord} />
        ))}
      </div>
    </section>
  );
}

// ── Where memory lives ────────────────────────────────────────────────────

/// One scope row, from the store totals or, when those are not available,
/// from the loaded records.
export interface ScopeRowModel {
  key: string;
  kind: string;
  scopeKey: string;
  realm: string;
  active: number;
  quarantined: number;
  superseded: number;
  tombstoned: number;
  bytes: number;
  floorPressure: boolean;
  filter: MemoryRecordsFilter;
}

export function scopeRowFromOverview(scope: MemoryScopeOverview): ScopeRowModel {
  return {
    key: overviewScopeKey(scope),
    kind: scope.scope_kind,
    scopeKey: scope.scope_key,
    realm: scope.realm,
    active: scope.active ?? 0,
    quarantined: scope.quarantined ?? 0,
    superseded: scope.superseded ?? 0,
    tombstoned: scope.tombstoned ?? 0,
    bytes: scope.body_bytes ?? 0,
    floorPressure: scope.floor_pressure === true,
    filter: filterForOverviewScope(scope),
  };
}

export function scopeRowFromLoaded(row: ScopeOverviewRow): ScopeRowModel {
  return {
    key: row.key,
    kind: row.scope.scope,
    scopeKey: scopeKeyOf(row.scope),
    realm: row.scope.realm,
    active: row.active,
    quarantined: row.quarantined,
    superseded: row.superseded,
    tombstoned: row.tombstoned,
    bytes: row.bytes,
    floorPressure: false,
    filter: filterForScope(row.scope),
  };
}

/// A scope's mark: the agent's initial, or the glyph of a team, an
/// operator or the shared space.
function ScopeMark({ kind, name }: { kind: string; name: string }): React.JSX.Element {
  if (kind === "identity") return <AgentAvatar name={name} size={22} />;
  return (
    <span className="memory-scope-glyph" data-kind={kind} aria-hidden="true">
      <KitGlyph name={kind === "mob" ? "roster" : kind === "operator" ? "person" : "memory"} />
    </span>
  );
}

/// What a scope holds as one bar: active in ink, quarantined in the warning
/// tone, replaced in mid ink, discarded faint, with 2 px gaps.
function CompositionBar({ row, maxRecords }: { row: ScopeRowModel; maxRecords: number }): React.JSX.Element {
  const parts = [
    { key: "active", value: row.active },
    { key: "quarantined", value: row.quarantined },
    { key: "superseded", value: row.superseded },
    { key: "tombstoned", value: row.tombstoned },
  ].filter((part) => part.value > 0);
  const total = parts.reduce((sum, part) => sum + part.value, 0);
  // The bar's length compares scopes: the largest scope fills the column.
  const share = maxRecords > 0 ? Math.max(0.04, total / maxRecords) : 0;
  return (
    <span className="memory-composition" role="img" aria-label={scopeCountsText(row)} style={{ width: `${share * 100}%` }}>
      {total > 0
        ? parts.map((part) => (
            <span
              key={part.key}
              className="memory-composition__seg"
              data-part={part.key}
              style={{ flexGrow: part.value }}
              title={`${part.value} ${statusWord({ status: part.key } as MemoryPanelRecord["status"]).toLowerCase()}`}
            />
          ))
        : null}
    </span>
  );
}

function landscapeItems(lane: LandscapeLane | undefined, now: number): { items: DotStripItem[]; realms: Map<string, string> } {
  const realms = new Map<string, string>();
  const items: DotStripItem[] = [];
  for (const tick of lane?.ticks ?? []) {
    realms.set(tick.id, tick.realm);
    const when = formatRelativeTime(tick.at, now);
    const state = tick.shape === "quarantined" ? "quarantined" : tick.shape === "faint" ? "replaced or discarded" : "";
    items.push({
      id: tick.id,
      t: tick.at,
      shape: tick.shape === "quarantined" ? "triangle" : "tick",
      tone: tick.shape === "quarantined" ? "warn" : undefined,
      faint: tick.shape === "faint",
      label: joinMeta(tick.title, state, when ? `written ${when}` : null),
    });
  }
  return { items, realms };
}

function LandscapeRow({
  row,
  lane,
  window,
  now,
  multiRealm,
  limitTitle,
  floorBytes,
  maxBytes,
  maxRecords,
  trackWidth,
  onOpen,
  onOpenRecord,
}: {
  row: ScopeRowModel;
  lane: LandscapeLane | undefined;
  /** The timeline track's width in px (measured once for every row). */
  trackWidth: number;
  window: { start: number; end: number };
  now: number;
  multiRealm: boolean;
  limitTitle: string;
  floorBytes: number | null;
  maxBytes: number;
  maxRecords: number;
  onOpen: () => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const nameOf = useNameOf();
  const agent = row.kind === "identity";
  const name = scopeOwnerName(row.kind, row.scopeKey, nameOf);
  const counts = scopeCountsText(row);
  const { items, realms } = landscapeItems(lane, now);
  const sizeMax = floorBytes && floorBytes > 0 ? floorBytes : Math.max(1, maxBytes);
  return (
    <div
      className={cx("memory-landscape__row", row.floorPressure && "is-near-limit")}
      data-testid={`memory-holdings-scope:${row.key}`}
      onClick={(event) => {
        const target = event.target as Element | null;
        if (target?.closest?.("button, a, input, select, [role='button']")) return;
        onOpen();
      }}
    >
      <span className="memory-landscape__who">
        <ScopeMark kind={row.kind} name={name} />
        <span className="memory-landscape__names">
          <button
            type="button"
            className="memory-landscape__name"
            onClick={onOpen}
            // A team reads by its name; the key it is filed under is the tooltip.
            title={row.kind === "mob" && row.scopeKey ? row.scopeKey : undefined}
          >
            {agent ? <AgentName id={row.scopeKey} /> : name}
          </button>
          <span className="memory-landscape__sub">
            <MetaLine
              parts={[
                scopeKindLabel(row.kind),
                multiRealm ? <span title={`Realm ${row.realm}`}>{realmSpaceName(row.realm)}</span> : null,
                counts,
              ]}
            />
          </span>
        </span>
      </span>
      <span className="memory-landscape__composition">
        <CompositionBar row={row} maxRecords={maxRecords} />
      </span>
      <span className="memory-landscape__timeline">
        <span
          className="memory-landscape__earlier"
          title={lane && lane.earlier > 0 ? `${plural(lane.earlier, "record")} written before this window` : undefined}
        >
          {lane && lane.earlier > 0 ? `+${lane.earlier}` : ""}
        </span>
        <span className="memory-landscape__track" data-testid={`memory-landscape-scope:${row.key}`}>
          <DotStrip
            className="memory-landscape__ticks"
            items={items}
            start={window.start}
            end={window.end}
            width={trackWidth}
            height={30}
            markSize={7}
            label={`${name}: ${plural(items.length, "record")} written in the last 30 days`}
            testIdPrefix="memory-landscape-tick"
            onSelect={(item) => onOpenRecord(realms.get(item.id), item.id)}
          />
        </span>
      </span>
      <span className="memory-landscape__size">
        {row.floorPressure ? (
          <Chip tone="warn" testId={`memory-holdings-floor:${row.key}`} title={limitTitle}>
            Near limit
          </Chip>
        ) : null}
        <MiniBar
          value={row.bytes}
          max={sizeMax}
          tone={row.floorPressure ? "warn" : undefined}
          width={44}
          label={floorBytes ? `${formatSize(row.bytes)} of the ${formatSize(floorBytes)} limit` : formatSize(row.bytes)}
          valueLabel={formatSize(row.bytes)}
        />
      </span>
    </div>
  );
}

/// Denied scopes vanish from the row-filtered listing; the one-row probes
/// make them render as no-grant rows instead of silently not existing. The
/// missing grant reads as a sentence, its name the tooltip.
function DeniedScopeRow({ kind }: { kind: "mob" | "operator" }): React.JSX.Element {
  const grant = `${kind}.memory.read`;
  return (
    <ListRow
      className="memory-landscape__denied"
      testId={`memory-holdings-scope-denied:${kind}`}
      lead={<KitGlyph name="shield" className="memory-scope-lock" />}
      title={kind === "mob" ? "Team memory" : "Operator memory"}
      subtitle={<span title={grantTitle(grant)}>{needsGrantText(grant)}</span>}
      meta={<Chip tone="locked">No access</Chip>}
    />
  );
}

/// The labelled week lines of the landscape axis: one, two and three weeks
/// back, by date (the track draws a line every week).
function axisWeeks(window: { start: number; end: number }): Array<{ x: number; label: string }> {
  const span = window.end - window.start;
  return [21, 14, 7].map((days) => {
    const at = window.end - days * DAY_MS;
    return {
      x: (at - window.start) / span,
      label: new Date(at).toLocaleDateString("en-GB", { day: "numeric", month: "short" }),
    };
  });
}

export function Landscape({
  rows,
  records,
  readComplete,
  readCount,
  overviewLoaded,
  multiRealm,
  floors,
  operatorScopeDenied,
  mobScopeDenied,
  now,
  onOpenScope,
  onOpenRecord,
}: {
  rows: ScopeRowModel[];
  /** Every record the console has read: ticks come from these. */
  records: ReadonlyArray<MemoryPanelRecord>;
  /** The read covers the whole store (the walk finished under its cap). */
  readComplete: boolean;
  readCount: number;
  overviewLoaded: boolean;
  multiRealm: boolean;
  floors?: { records?: number; bytes?: number };
  operatorScopeDenied: boolean;
  mobScopeDenied: boolean;
  now: number;
  onOpenScope: (filter: MemoryRecordsFilter) => void;
  onOpenRecord: (realm: string | undefined, memoryId: string) => void;
}): React.JSX.Element {
  const window = React.useMemo(() => landscapeWindow(now), [now]);
  const [trackRef, trackWidth] = useMeasuredWidth<HTMLSpanElement>(480);
  const lanes = React.useMemo(() => landscapeLanes(records, window), [records, window]);
  const weeks = React.useMemo(() => axisWeeks(window), [window]);
  const totalBytes = rows.reduce((sum, row) => sum + row.bytes, 0);
  const maxBytes = Math.max(1, ...rows.map((row) => row.bytes));
  const maxRecords = Math.max(1, ...rows.map((row) => row.active + row.quarantined + row.superseded + row.tombstoned));
  const limitTitle = floors
    ? `Close to the per-scope limit of ${floors.records ?? "?"} records or ${
        typeof floors.bytes === "number" ? formatSize(floors.bytes) : "?"
      }.`
    : "Close to the per-scope limit.";
  return (
    <section className="memory-block memory-landscape" data-testid="memory-landscape" aria-labelledby="memory-landscape-head">
      <SectionHeader
        id="memory-landscape-head"
        label="Where memory lives"
        count={rows.length > 0 ? rows.length : null}
        action={rows.length > 0 ? <span className="memory-block__meta">{`${formatSize(totalBytes)} in total`}</span> : undefined}
      />
      {!overviewLoaded ? (
        <p className="memory-caption">{`Counts cover the ${plural(records.length, "loaded record")}, not the whole store.`}</p>
      ) : null}
      {rows.length === 0 && !operatorScopeDenied && !mobScopeDenied ? (
        <EmptyState inline title="No memory records yet." body="Agents and the memory steward add facts, preferences and procedures here as they work." />
      ) : (
        <div className="memory-landscape__grid">
          <div className="memory-landscape__axis" aria-hidden="true">
            <span className="memory-landscape__axis-who" />
            <span className="memory-landscape__axis-composition">Holds</span>
            <span className="memory-landscape__axis-timeline">
              <span className="memory-landscape__axis-gutter" />
              <span className="memory-landscape__axis-track" ref={trackRef}>
                <span className="memory-landscape__axis-start">30 days ago</span>
                {weeks.map((week) => (
                  <span key={week.label} className="memory-landscape__axis-week" style={{ left: `${week.x * 100}%` }}>
                    {week.label}
                  </span>
                ))}
                <span className="memory-landscape__axis-end">today</span>
              </span>
            </span>
            <span className="memory-landscape__axis-size">Size</span>
          </div>
          {rows.map((row) => (
            <LandscapeRow
              key={row.key}
              row={row}
              lane={lanes.get(row.key)}
              window={window}
              now={now}
              multiRealm={multiRealm}
              limitTitle={limitTitle}
              floorBytes={typeof floors?.bytes === "number" ? floors.bytes : null}
              maxBytes={maxBytes}
              maxRecords={maxRecords}
              trackWidth={trackWidth}
              onOpen={() => onOpenScope(row.filter)}
              onOpenRecord={onOpenRecord}
            />
          ))}
          {mobScopeDenied ? <DeniedScopeRow kind="mob" /> : null}
          {operatorScopeDenied ? <DeniedScopeRow kind="operator" /> : null}
        </div>
      )}
      {rows.length > 0 ? (
        <div className="memory-landscape__foot">
          <span className="memory-landscape__legend" aria-hidden="true">
            <span className="memory-legend" data-part="active">active</span>
            <span className="memory-legend" data-part="quarantined">quarantined</span>
            <span className="memory-legend" data-part="faint">replaced or discarded</span>
          </span>
          <span className="memory-landscape__coverage">
            {readComplete
              ? `Each tick is a record, placed when it was written; ${plural(readCount, "record")} read.`
              : `Each tick is a record, placed when it was written. Latest ${Math.min(readCount, LATTICE_WALK_MAX_RECORDS)} records.`}
          </span>
        </div>
      ) : null}
    </section>
  );
}

// ── In progress and retired agents ────────────────────────────────────────

export function InProgress({
  dreams,
  dreamsDenied,
  dreamsWithheld,
  canReviewQuarantine,
  quarantineRecords,
  pendingPromotions,
  proposals,
  proposalsDenied,
  onOpenPipeline,
  onOpenDreams,
}: {
  dreams: MemoryDreamRun[];
  dreamsDenied: boolean;
  dreamsWithheld?: boolean;
  canReviewQuarantine: boolean;
  quarantineRecords: MemoryPanelRecord[];
  pendingPromotions: MemoryPendingPromotion[];
  proposals: MemoryProposalEntry[];
  proposalsDenied: boolean;
  onOpenPipeline: () => void;
  onOpenDreams: () => void;
}): React.JSX.Element {
  const tainted = proposals.filter((proposal) => proposal.tainted).length;
  const lastDream = dreams[0];
  const lastDreamAt = lastDream ? lastDream.last_op_at_ms || lastDream.first_op_at_ms : undefined;
  const waiting = quarantineRecords.length + pendingPromotions.length;
  return (
    <section className="memory-block" aria-labelledby="memory-progress-head">
      <SectionHeader id="memory-progress-head" label="In progress" />
      <div className="memory-progress">
        <div className="memory-progress__card">
          <span className="memory-progress__label">Last dream</span>
          <span className="memory-progress__value">
            {lastDream ? (
              <MetaLine
                parts={[
                  lastDreamAt ? <RelativeTime value={lastDreamAt} /> : "Time unknown",
                  typeof lastDream.ops === "number" ? plural(lastDream.ops, "change") : null,
                ]}
              />
            ) : dreamsDenied ? (
              "No access"
            ) : dreamsWithheld ? (
              "Withheld by access"
            ) : (
              "No dream has run yet"
            )}
          </span>
          {lastDream ? (
            <button type="button" className="memory-link memory-progress__link" onClick={onOpenDreams}>
              See the dreams
            </button>
          ) : null}
        </div>
        <div className={cx("memory-progress__card", canReviewQuarantine && waiting > 0 && "is-attention")}>
          <span className="memory-progress__label">Quarantine</span>
          {canReviewQuarantine ? (
            <span className="memory-progress__value memory-progress__value--action">
              <span>{`${plural(quarantineRecords.length, "record")}, ${pendingPromotions.length} waiting for approval`}</span>
              <Button size="sm" onClick={onOpenPipeline}>
                Open pipeline
              </Button>
            </span>
          ) : (
            <span className="memory-progress__value" title={grantTitle("memory.quarantine.review")}>
              {needsGrantText("memory.quarantine.review")}
            </span>
          )}
        </div>
        <div className="memory-progress__card">
          <span className="memory-progress__label">Proposals</span>
          <span className="memory-progress__value">
            {proposalsDenied
              ? "No access"
              : `${proposals.length} waiting${
                  tainted > 0
                    ? `, ${tainted} held because ${tainted === 1 ? "its source was" : "their sources were"} untrusted`
                    : ""
                }`}
          </span>
        </div>
      </div>
    </section>
  );
}

function HarvestRow({ harvest }: { harvest: MemoryHarvestEntry }): React.JSX.Element {
  const nameOf = useNameOf();
  const name = nameOf(harvest.identity) || harvest.identity;
  const cause =
    harvest.cause && harvest.cause.trim().toLowerCase() !== "retired" ? humanizeEnum(harvest.cause) : "";
  return (
    <ListRow
      testId={`memory-harvest:${harvest.identity}`}
      lead={<AgentAvatar name={name} />}
      title={<AgentName id={harvest.identity} link />}
      subtitle={
        harvest.retired_at_ms ? (
          <RelativeTime
            value={harvest.retired_at_ms}
            prefix="Retired"
            title={joinMeta(
              formatAbsoluteTime(harvest.retired_at_ms),
              harvest.session_key ? `session ${harvest.session_key}` : null,
            )}
          />
        ) : (
          "Retired"
        )
      }
      meta={cause ? <Chip>{cause}</Chip> : <span className="memory-harvest__wait">Awaiting its summary</span>}
    />
  );
}

export function Harvests({ harvests, denied }: { harvests: MemoryHarvestEntry[]; denied: boolean }): React.JSX.Element {
  return (
    <section className="memory-block" data-testid="memory-harvests" aria-labelledby="memory-harvests-head">
      <SectionHeader
        id="memory-harvests-head"
        label="Retired agents awaiting their summary"
        count={!denied && harvests.length > 0 ? harvests.length : null}
      />
      {denied ? (
        <EmptyState inline kind="no-access" title="Retired agents: no access." />
      ) : harvests.length === 0 ? (
        <EmptyState inline title="No retired agent is waiting." />
      ) : (
        <RowList>
          {harvests.map((harvest, index) => (
            <HarvestRow key={`${harvest.realm}:${harvest.identity}:${index}`} harvest={harvest} />
          ))}
        </RowList>
      )}
    </section>
  );
}

/// The Holdings scope rows: store totals when the overview answered, else
/// the loaded records grouped by scope.
export function holdingsRows(
  overviewScopes: MemoryScopeOverview[] | null,
  records: MemoryPanelRecord[],
): ScopeRowModel[] {
  return overviewScopes ? overviewScopes.map(scopeRowFromOverview) : scopeOverviewRows(records).map(scopeRowFromLoaded);
}

export function HoldingsDenied(): React.JSX.Element {
  return (
    <Notice tone="locked" testId="memory-holdings-denied">
      Records: no access. Your access does not include reading memory records.
    </Notice>
  );
}

export function HoldingsLoading(): React.JSX.Element {
  return <LoadingState label="Loading memory" rows={4} />;
}

export type { MemoryRecordScope };
