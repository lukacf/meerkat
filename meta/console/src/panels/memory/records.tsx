// Records (L1): the filter bar (server filters by agent or team, scope,
// status and space; client refinements by text, kind and trust over the
// records the console has read) and three views of the same records: List
// (grouped by scope when nothing narrows it), Timeline (day groups on a
// spine, versions linked) and Clusters (one tile per kind).

import React from "react";

import type { MemoryPanelRecord, MemoryRecordKind } from "../../types";
import {
  AgentName,
  Button,
  EmptyState,
  FilterBar,
  LoadingState,
  Notice,
  RowList,
  SearchField,
  SectionHeader,
  SegmentedControl,
  SelectField,
  Toolbar,
  TrustLadder,
  cx,
  plural,
  type SelectOption,
} from "./kit";
import {
  DEAD_INJECTION_THRESHOLD,
  hasActiveFilter,
  realmOfRecord,
  sortRecordsByUtility,
  type MemoryRecordDetail,
  type MemoryRecordsFilter,
  type RecordsListView,
} from "./model";
import { MemoryRecordRow, kindGlyph, useNameOf } from "./parts";
import { RecordsClusters } from "./records-clusters";
import { RecordsTimeline } from "./records-timeline";
import {
  MEMORY_KINDS,
  hasRefinement,
  mergeReadRecords,
  recordChangedAt,
  refineRecords,
  trustLevel,
  type RecordsRefinement,
} from "./viz-model";
import { STATUS_WORD, kindLabel, realmSpaceName, scopeHeading } from "./words";

export type RecordsView = "list" | "timeline" | "clusters";

const SCOPE_FILTER_OPTIONS: SelectOption[] = [
  { value: "", label: "All" },
  { value: "identity", label: "Agents" },
  { value: "mob", label: "Teams" },
  { value: "operator", label: "Operators" },
  { value: "realm", label: "Shared" },
];

const STATUS_FILTER_OPTIONS: SelectOption[] = [
  { value: "", label: "All" },
  { value: "active", label: STATUS_WORD.active },
  { value: "quarantined", label: STATUS_WORD.quarantined },
  { value: "superseded", label: STATUS_WORD.superseded },
  { value: "tombstoned", label: STATUS_WORD.tombstoned },
];

const SORT_OPTIONS: SelectOption[] = [
  { value: "recency", label: "Recent" },
  { value: "utility", label: "Least useful" },
];

const TRUST_STEPS: Array<{ level: number; label: string }> = [
  { level: 1, label: "Untrusted" },
  { level: 2, label: "Observed" },
  { level: 3, label: "Verified" },
  { level: 4, label: "Application" },
  { level: 5, label: "Operator" },
];

/// Rows a refined list renders before "Show more".
const REFINED_PAGE = 200;


export interface RecordsTabProps {
  listView: RecordsListView;
  /** Every record the console has read (the base page, load-more pages
   * and the trust rules walk): the search corpus when no server filter is
   * set. */
  readRecords: ReadonlyArray<MemoryPanelRecord>;
  filter: MemoryRecordsFilter;
  onFilterEdit: (next: MemoryRecordsFilter) => void;
  onFilterApply: (next: MemoryRecordsFilter) => void;
  onFilterBlur: () => void;
  sortMode: "recency" | "utility";
  onSortMode: (mode: "recency" | "utility") => void;
  view: RecordsView;
  onView: (view: RecordsView) => void;
  refinement: RecordsRefinement;
  onRefinement: (next: RecordsRefinement) => void;
  realms: string[];
  loaded: boolean;
  pageLoading: boolean;
  recordsDenied: boolean;
  canQuery: boolean;
  detail: MemoryRecordDetail | null;
  now: number;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
  onLoadMore: () => void;
}

export function RecordsTab(props: RecordsTabProps): React.JSX.Element {
  const {
    listView,
    readRecords,
    filter,
    sortMode,
    view,
    refinement,
    realms,
    loaded,
    pageLoading,
    recordsDenied,
    canQuery,
    detail,
    now,
    onSelectRecord,
  } = props;
  const nameOf = useNameOf();
  const multiRealm = realms.length > 1;
  const filtered = hasActiveFilter(filter);
  const refined = hasRefinement(refinement);
  const [shown, setShown] = React.useState(REFINED_PAGE);
  React.useEffect(() => setShown(REFINED_PAGE), [refinement, filter, sortMode]);

  // The search corpus: the server-filtered page while a server filter is
  // set; otherwise everything read, so a search reaches past the first page.
  const corpus = React.useMemo(
    () => (filtered ? listView.records : mergeReadRecords(listView.records, readRecords)),
    [filtered, listView.records, readRecords],
  );
  const refinedRecords = React.useMemo(() => {
    const matches = refineRecords(corpus, refinement);
    return sortMode === "utility"
      ? sortRecordsByUtility(matches)
      : [...matches].sort((a, b) => (recordChangedAt(b) ?? 0) - (recordChangedAt(a) ?? 0));
  }, [corpus, refinement, sortMode]);
  const kindCounts = React.useMemo(() => {
    const base = refineRecords(corpus, { ...refinement, kinds: [] });
    const counts = new Map<MemoryRecordKind, number>();
    for (const record of base) counts.set(record.kind, (counts.get(record.kind) ?? 0) + 1);
    return counts;
  }, [corpus, refinement]);
  const trustCounts = React.useMemo(() => {
    const base = refineRecords(corpus, { ...refinement, trust: [] });
    const counts = new Map<number, number>();
    for (const record of base) counts.set(trustLevel(record.trust), (counts.get(trustLevel(record.trust)) ?? 0) + 1);
    return counts;
  }, [corpus, refinement]);

  const toggleKind = (kind: MemoryRecordKind) =>
    props.onRefinement({
      ...refinement,
      kinds: refinement.kinds.includes(kind) ? refinement.kinds.filter((entry) => entry !== kind) : [...refinement.kinds, kind],
    });
  const toggleTrust = (level: number) =>
    props.onRefinement({
      ...refinement,
      trust: refinement.trust.includes(level) ? refinement.trust.filter((entry) => entry !== level) : [...refinement.trust, level],
    });

  // The records the Timeline and the flat list show, in display order.
  const flatRecords = refined ? refinedRecords : listView.records;
  const openId = detail?.record.id;
  const empty = loaded && !pageLoading && (refined ? refinedRecords.length === 0 : listView.records.length === 0);

  // The view switch rides at the end of the filter row (it wraps with it
  // in a narrow pane, where it keeps its glyphs and drops the words).
  const viewWord = (word: string) => <span className="memory-records__view-word">{word}</span>;
  const viewSwitch = (
    <SegmentedControl<RecordsView>
      className="memory-records__views"
      ariaLabel="Records view"
      value={view}
      onChange={props.onView}
      options={[
        { id: "list", label: viewWord("List"), ariaLabel: "List", title: "List", glyph: "list", testId: "memory-records-view:list" },
        { id: "timeline", label: viewWord("Timeline"), ariaLabel: "Timeline", title: "Timeline", glyph: "calendar", testId: "memory-records-view:timeline" },
        { id: "clusters", label: viewWord("Clusters"), ariaLabel: "Clusters", title: "Clusters", glyph: "clusters", testId: "memory-records-view:clusters" },
      ]}
    />
  );

  return (
    <div className="memory-tab memory-records" data-view={view}>
      <div className="memory-records__bar">
        {/* What to load: the server's filters, the order, and the view. */}
        <div className="memory-records__row">
          {canQuery ? (
            <Toolbar testId="memory-filter" className="memory-toolbar">
              <SearchField
                label="Search by agent"
                placeholder="Agent or team"
                value={filter.key || ""}
                testId="memory-filter-input"
                className="memory-toolbar__agent"
                onChange={(value) => props.onFilterEdit({ ...filter, key: value })}
                onKeyDown={(event) => {
                  if (event.key === "Enter") props.onFilterApply(filter);
                }}
                onBlur={() => props.onFilterBlur()}
              />
              <SelectField
                label="Scope"
                variant="chip"
                value={filter.scope || ""}
                options={SCOPE_FILTER_OPTIONS}
                testId="memory-filter:scope"
                className="memory-toolbar__select"
                onChange={(value) =>
                  props.onFilterApply({ ...filter, scope: (value || undefined) as MemoryRecordsFilter["scope"] })
                }
              />
              <SelectField
                label="Status"
                variant="chip"
                value={filter.status || ""}
                options={STATUS_FILTER_OPTIONS}
                testId="memory-filter:status"
                className="memory-toolbar__select"
                onChange={(value) =>
                  props.onFilterApply({ ...filter, status: (value || undefined) as MemoryRecordsFilter["status"] })
                }
              />
              {multiRealm ? (
                // Realms read as spaces: the stores of state this gateway serves.
                <SelectField
                  label="Space"
                  variant="chip"
                  value={filter.realm || ""}
                  options={[{ value: "", label: "All spaces" }, ...realms.map((realm) => ({ value: realm, label: realm }))]}
                  testId="memory-filter:realm"
                  className="memory-toolbar__select"
                  onChange={(value) => props.onFilterApply({ ...filter, realm: value || undefined })}
                />
              ) : null}
              <SelectField
                label="Sort"
                variant="chip"
                value={sortMode}
                options={SORT_OPTIONS}
                testId="memory-sort"
                className="memory-toolbar__select"
                onChange={(value) => props.onSortMode(value === "utility" ? "utility" : "recency")}
              />
              {filtered ? (
                <Button variant="ghost" size="sm" data-testid="memory-filter-clear" onClick={() => props.onFilterApply({})}>
                  Clear filters
                </Button>
              ) : null}
              {viewSwitch}
            </Toolbar>
          ) : null}
          {canQuery ? null : viewSwitch}
        </div>
        {/* What to show of what is loaded: text, kind and trust, in the browser. */}
        <FilterBar
          className="memory-refine"
          ariaLabel="Refine the loaded records"
          search={{
            label: "Find in loaded records",
            placeholder: "Find in titles, descriptions and tags",
            value: refinement.text,
            testId: "memory-records-search",
            onChange: (value) => props.onRefinement({ ...refinement, text: value }),
          }}
          // No `key` on the chips: the bar keys them by their test ids (a
          // key spread into JSX warns in React 19).
          chips={[
            ...MEMORY_KINDS.map((kind) => ({
              glyph: kindGlyph(kind),
              label: kindLabel(kind),
              count: kindCounts.get(kind) ?? 0,
              pressed: refinement.kinds.includes(kind),
              onToggle: () => toggleKind(kind),
              testId: `memory-records-kind:${kind}`,
            })),
            ...TRUST_STEPS.filter((step) => (trustCounts.get(step.level) ?? 0) > 0 || refinement.trust.includes(step.level)).map(
              (step, index) => ({
                className: cx("memory-refine__trust", index === 0 && "is-first"),
                label: (
                  <span className="memory-chip-label">
                    <TrustLadder level={step.level} label={step.label} className="memory-ladder--bare" />
                    {step.label}
                  </span>
                ),
                ariaLabel: `${step.label} trust, ${plural(trustCounts.get(step.level) ?? 0, "record")}`,
                count: trustCounts.get(step.level) ?? 0,
                pressed: refinement.trust.includes(step.level),
                onToggle: () => toggleTrust(step.level),
                testId: `memory-records-trust:${step.level}`,
              }),
            ),
          ]}
        >
          {refined ? (
            <Button variant="ghost" size="sm" data-testid="memory-records-refine-clear" onClick={() => props.onRefinement({ text: "", kinds: [], trust: [] })}>
              Clear
            </Button>
          ) : null}
        </FilterBar>
      </div>

      {sortMode === "utility" ? (
        <Notice testId="memory-utility-note">
          {`Least useful first: records given to agents at least ${DEAD_INJECTION_THRESHOLD} times and never judged useful lead, marked Never useful. Space spent is estimated from how often each was given and its size.`}
        </Notice>
      ) : null}
      {multiRealm && !filter.realm?.trim() ? (
        <Notice testId="memory-multi-realm-note">
          This page combines records from several spaces. Pick a space to page through its records.
        </Notice>
      ) : null}
      {!loaded || pageLoading ? <LoadingState label="Loading records" rows={4} /> : null}
      {empty ? (
        recordsDenied || listView.denied ? (
          <EmptyState
            kind="no-access"
            testId="memory-records-empty"
            title="Records: no access."
            body="Your access does not include reading these memory records."
          />
        ) : filtered || refined ? (
          <EmptyState
            kind="filtered"
            testId="memory-records-empty"
            title={refined ? "No loaded record matches." : "No records match these filters."}
            action={
              <Button
                onClick={() => {
                  if (refined) props.onRefinement({ text: "", kinds: [], trust: [] });
                  if (filtered) props.onFilterApply({});
                }}
              >
                Clear filters
              </Button>
            }
          />
        ) : (
          <EmptyState
            testId="memory-records-empty"
            title="No memory records yet."
            body="Agents and the memory steward add facts, preferences and procedures here as they work."
          />
        )
      ) : null}
      {loaded && !pageLoading && listView.denied && listView.records.length > 0 ? (
        <Notice tone="locked" testId="memory-records-denied-note">
          Further pages: no access. Loading more of this list was denied for your access.
        </Notice>
      ) : null}

      {loaded && !pageLoading && !empty ? (
        view === "clusters" ? (
          <RecordsClusters
            records={refineRecords(corpus, { ...refinement, kinds: [] })}
            selectedKinds={refinement.kinds}
            onPick={(kind) => {
              props.onRefinement({ ...refinement, kinds: [kind] });
              props.onView("list");
            }}
            onOpenRecord={onSelectRecord}
          />
        ) : view === "timeline" ? (
          <RecordsTimeline
            records={flatRecords}
            now={now}
            openId={openId}
            query={refinement.text}
            onSelectRecord={onSelectRecord}
          />
        ) : refined ? (
          <section className="cc-section memory-record-group">
            <SectionHeader label="Matching records" count={refinedRecords.length} />
            <RowList>
              {refinedRecords.slice(0, shown).map((record) => (
                <MemoryRecordRow
                  key={record.id}
                  record={record}
                  showScope
                  query={refinement.text}
                  utilityMode={sortMode === "utility"}
                  selected={record.id === openId}
                  onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                />
              ))}
            </RowList>
            {refinedRecords.length > shown ? (
              <div className="memory-load-more">
                <Button onClick={() => setShown((count) => count + REFINED_PAGE)}>
                  {`Show ${Math.min(REFINED_PAGE, refinedRecords.length - shown)} more`}
                </Button>
              </div>
            ) : null}
          </section>
        ) : listView.mode === "flat" ? (
          <section className="cc-section memory-record-group">
            <SectionHeader
              label={sortMode === "utility" && !filtered ? "Least useful first" : "Matching records"}
              count={listView.records.length}
            />
            <RowList>
              {listView.records.map((record) => (
                <MemoryRecordRow
                  key={record.id}
                  record={record}
                  showScope
                  utilityMode={sortMode === "utility"}
                  selected={record.id === openId}
                  onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                />
              ))}
            </RowList>
          </section>
        ) : (
          listView.groups.map((group) => (
            <section className="cc-section memory-record-group" key={group.key} data-testid={`memory-group:${group.key}`}>
              <SectionHeader
                className="memory-record-group__head"
                label={
                  <>
                    {group.scope.scope === "identity" ? (
                      <AgentName id={group.scope.identity} link />
                    ) : group.scope.scope === "mob" ? (
                      <span title={group.scope.mob}>{scopeHeading(group.scope, nameOf)}</span>
                    ) : (
                      scopeHeading(group.scope, nameOf)
                    )}
                    {multiRealm ? (
                      <span title={`Realm ${group.scope.realm}`}>{` · ${realmSpaceName(group.scope.realm)}`}</span>
                    ) : null}
                  </>
                }
                count={group.records.length}
              />
              <RowList>
                {group.records.map((record) => (
                  <MemoryRecordRow
                    key={record.id}
                    record={record}
                    selected={record.id === openId}
                    onSelect={() => onSelectRecord(realmOfRecord(record), record.id)}
                  />
                ))}
              </RowList>
            </section>
          ))
        )
      ) : null}

      {listView.cursor && canQuery ? (
        <div className="memory-load-more">
          <Button data-testid="memory-load-more" disabled={pageLoading} onClick={props.onLoadMore}>
            {refined ? "Search more records" : "Load more records"}
          </Button>
          <span className="memory-load-more__note">
            {refined
              ? `Searching ${plural(corpus.length, "loaded record")}.`
              : `${plural(listView.records.length, "record")} loaded.`}
          </span>
        </div>
      ) : null}
    </div>
  );
}

