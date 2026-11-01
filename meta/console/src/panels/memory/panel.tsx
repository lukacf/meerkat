// The Memory panel: what the team knows, where each memory came from, how it
// is used, and what needs a human decision. L0 is the header's live summary
// and the glance strip; L1 the five tabs; L2 a record's biography in the
// drawer; L3 the technical details.

import React from "react";

import type { ConversationTimelineEntry } from "@console-core";
import type {
  ConsoleFrame,
  MemoryAuditVerdictEntry,
  MemoryDreamRun,
  MemoryDreamRunSheet,
  MemoryEvidenceRef,
  MemoryFullRecord,
  MemoryHarvestEntry,
  MemoryLedgerEntry,
  MemoryPanelOverviewResult,
  MemoryPanelRecord,
  MemoryPanelRecordsResult,
  MemoryPendingPromotion,
  MemoryProposalEntry,
  MemoryQuarantineDecideResult,
  MemoryQuarantineVerdict,
} from "../../types";
import { Biography } from "./biography";
import { DreamsTab } from "./dreams";
import { MemoryGlance, memorySummaryFacts, type GlanceInputs } from "./glance";
import {
  ChecksStrip,
  Harvests,
  HoldingsDenied,
  HoldingsLoading,
  InProgress,
  Landscape,
  holdingsRows,
} from "./holdings";
import {
  DrawerLayout,
  EmptyState,
  IconButton,
  KitGlyph,
  LoadingState,
  PanelBody,
  PanelHeader,
  PanelShell,
  PanelTabs,
  StatusRow,
  agentDisplayName,
  formatRelativeTime,
  standardPanelMenu,
  useAgentDirectory,
  useNow,
  usePanelDensity,
  useShowTechnicalIds,
  type PanelTab,
} from "./kit";
import { KnowledgeTab } from "./knowledge";
import {
  MEMORY_TABS,
  annotateInjectionDups,
  buildRecordsListView,
  computeVerdictTiles,
  createMemoryRecordsPager,
  dedupeAuditVerdicts,
  dreamRunsNewestFirst,
  identityOptions,
  latticeFingerprint,
  memoryTabLabel,
  resolveMemoryTabAlias,
  resolveRecordsSearchKey,
  runLatticeWalk,
  sortOverviewScopes,
  visibleForScopeKinds,
  visibleOverviewScopes,
  type LatticeWalkResult,
  type MemoryPagedState,
  type MemoryRecordDetail,
  type MemoryRecordsFilter,
  type MemoryTab,
  type VerdictTile,
} from "./model";
import { useNameOf } from "./parts";
import { PipelineTab, type StageModel } from "./pipeline";
import { quarantineDecisionSummary, type QuarantineNotice } from "./quarantine";
import { RecordsTab, type RecordsView } from "./records";
import {
  NO_REFINEMENT,
  dreamDecisions,
  loadedTotals,
  mergeReadRecords,
  storeTotals,
  type RecordsRefinement,
} from "./viz-model";
import { overviewScopeName, teamName } from "./words";

/// Where the Meta docs explain wiring a memory store into a runtime.
const MEMORY_DOCS_URL = "https://docs.rkat.ai/meta/concepts/memory#console-memory-panel";

export interface MemoryPanelProps {
  /** The team picker, when the console serves more than one team; shown
   * as the scope chip next to the title. */
  runtimeControl?: React.ReactNode;
  records: MemoryPanelRecord[];
  realms: string[];
  quarantineRecords: MemoryPanelRecord[];
  pendingPromotions: MemoryPendingPromotion[];
  dreams: MemoryDreamRun[];
  detail: MemoryRecordDetail | null;
  detailLoading?: boolean;
  canReviewQuarantine?: boolean;
  /// Memory is not set up on this runtime (no memory store): the panel
  /// shows the not-set-up state instead of empty sections.
  unavailable?: boolean;
  error?: string | null;
  /// False while the panel renders before its first records page has
  /// landed, when `records` and `realms` are an empty placeholder rather than
  /// the store's listing. The lattice page-walk waits for the listing: a walk
  /// over the placeholder would settle a verdict before the records arrive,
  /// and their arrival would then walk the same store again.
  loaded?: boolean;
  /// Keyset cursor from the initial records page.
  nextCursor?: string | null;
  /// Per-section -32030 outcomes: the section data is empty because the
  /// principal lacks the grant, not because the store is empty. Tiles whose
  /// evidence lives behind a denied section render "no access", never green.
  recordsDenied?: boolean;
  dreamsDenied?: boolean;
  /// The dreams and dream_runs reads said access withheld some realm's runs
  /// (withheld_by_access): never shown as "no dream yet".
  dreamsWithheld?: boolean;
  dreamRunsWithheld?: boolean;
  /// Scope-probe outcomes (§3.1/§5 ABAC): the unfiltered listing row-filters
  /// denied scopes silently, so a one-row probe per restricted scope kind is
  /// the only honest way to know a scope exists but is not readable. When
  /// set, Holdings renders the access-denied-tone scope row.
  operatorScopeDenied?: boolean;
  mobScopeDenied?: boolean;
  /// Phase-2 read surfaces. Each section carries its own -32030 outcome so a
  /// denied surface renders "no access", never an indistinguishable empty one.
  overview?: MemoryPanelOverviewResult | null;
  overviewDenied?: boolean;
  proposals?: MemoryProposalEntry[];
  proposalsDenied?: boolean;
  injections?: MemoryLedgerEntry[];
  injectionsDenied?: boolean;
  harvests?: MemoryHarvestEntry[];
  harvestsDenied?: boolean;
  dreamRuns?: MemoryDreamRunSheet[];
  dreamRunsDenied?: boolean;
  auditVerdicts?: MemoryAuditVerdictEntry[];
  auditVerdictsDenied?: boolean;
  /// Live `memory.*` frames from the in-memory ring (lossy: 1024/identity,
  /// 4096 total). Used only as freshness signals and pivot points.
  liveFrames?: ConsoleFrame[];
  onRefresh: (record?: { realm: string; memoryId: string }) => void;
  onSelectRecord: (realm: string | undefined, memoryId: string) => void;
  onClearDetail: () => void;
  /// Issue a filtered/paged panel/records query. Resolves null on access
  /// denial (-32030), mirroring the per-section tolerance of the base load.
  onQueryRecords?: (params: Record<string, unknown>) => Promise<MemoryPanelRecordsResult | null>;
  /// Resolve an evidence ref against meta/console/query_timeline. Resolves
  /// null when the session is no longer in the timeline (the degrade rule:
  /// fall back to the evidenceLabel text).
  onLoadEvidence?: (
    identity: string | undefined,
    evidence: MemoryEvidenceRef,
  ) => Promise<ConversationTimelineEntry[] | null>;
  /// Deep-link into the gating inbox: memory promotion verdicts ride the
  /// normal gating flow, never a parallel decision surface here.
  onOpenGating?: () => void;
  /// The operator half of quarantine review (meta/memory/quarantine/decide)
  /// for quarantined records that no gated promotion is waiting on. Passed
  /// only when the caller is advertised the method on a mutable console;
  /// absent, quarantined records stay read-only here. Rejects with the
  /// transport's typed JSON-RPC error.
  onDecideQuarantine?: (
    realm: string,
    record: MemoryFullRecord,
    verdict: MemoryQuarantineVerdict,
    rationale: string | undefined,
  ) => Promise<MemoryQuarantineDecideResult>;
}

/// The not-set-up state (A3.7): what Memory would show, and how to turn it on.
function MemoryNotSetUp({ runtimeControl }: { runtimeControl?: React.ReactNode }): React.JSX.Element {
  return (
    <PanelShell className="memory-panel memory-panel--not-set-up" testId="memory-panel">
      <PanelHeader title="Memory" scope={runtimeControl || undefined} />
      <div className="memory-not-set-up" data-testid="memory-unavailable">
        <EmptyState
          kind="not-configured"
          illustration="memory"
          testId="memory-not-set-up"
          title="Memory isn't set up on this runtime."
          body="Agents remember facts, preferences and procedures here once a memory store is configured. Each record shows where it came from, how agents use it, and what needs a review."
          action={
            <a className="memory-docs-link" href={MEMORY_DOCS_URL} target="_blank" rel="noopener noreferrer">
              How to set up memory
              <KitGlyph name="open" />
            </a>
          }
        />
      </div>
    </PanelShell>
  );
}

export function MemoryPanel({
  runtimeControl,
  records,
  realms,
  quarantineRecords,
  pendingPromotions: allPendingPromotions,
  dreams,
  detail,
  detailLoading = false,
  canReviewQuarantine = false,
  unavailable = false,
  error,
  loaded = true,
  nextCursor = null,
  recordsDenied = false,
  dreamsDenied = false,
  dreamsWithheld = false,
  dreamRunsWithheld = false,
  operatorScopeDenied = false,
  mobScopeDenied = false,
  overview = null,
  overviewDenied = false,
  proposals: allProposals = [],
  proposalsDenied = false,
  injections = [],
  injectionsDenied = false,
  harvests = [],
  harvestsDenied = false,
  dreamRuns = [],
  dreamRunsDenied = false,
  auditVerdicts = [],
  auditVerdictsDenied = false,
  liveFrames = [],
  onRefresh,
  onSelectRecord,
  onClearDetail,
  onQueryRecords,
  onLoadEvidence,
  onOpenGating,
  onDecideQuarantine,
}: MemoryPanelProps): React.JSX.Element {
  const nameOf = useNameOf();
  const now = useNow();
  // Rows of a scope kind the probes proved unreadable never show, whatever
  // an unfiltered read returned.
  const deniedKinds = { operatorScopeDenied, mobScopeDenied };
  const proposals = visibleForScopeKinds(allProposals, deniedKinds);
  const pendingPromotions = visibleForScopeKinds(allPendingPromotions, deniedKinds);
  const [tab, setTab] = React.useState<MemoryTab>("holdings");
  const tabIds = React.useId();
  const [density, setDensity] = usePanelDensity("memory");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("memory");
  // The outcome of the last quarantine decision, shown on the Biography it
  // navigated to (the released record after a release, the origin otherwise).
  const [decisionNotice, setDecisionNotice] = React.useState<QuarantineNotice | null>(null);
  const decideQuarantine = onDecideQuarantine
    ? async (record: MemoryFullRecord, verdict: MemoryQuarantineVerdict, rationale: string | undefined): Promise<void> => {
        if (!detail) return;
        const realm = detail.realm;
        const result = await onDecideQuarantine(realm, record, verdict, rationale);
        setDecisionNotice(quarantineDecisionSummary(result));
        onSelectRecord(realm, result.successor?.memory_id ?? record.id);
      }
    : undefined;
  const [filter, setFilter] = React.useState<MemoryRecordsFilter>({});
  const [sortMode, setSortMode] = React.useState<"recency" | "utility">("recency");
  const [recordsView, setRecordsView] = React.useState<RecordsView>("list");
  const [refinement, setRefinement] = React.useState<RecordsRefinement>(NO_REFINEMENT);
  const [paged, setPaged] = React.useState<MemoryPagedState | null>(null);
  const [pageLoading, setPageLoading] = React.useState(false);
  const queryRecordsRef = React.useRef(onQueryRecords);
  queryRecordsRef.current = onQueryRecords;
  // Names people can type in the records search: each agent scope key the
  // panel has seen, by its display name, and each team key by the team name
  // the headings show.
  const searchDirectory = useAgentDirectory();
  const searchCandidatesRef = React.useRef<Array<{ key: string; name: string }>>([]);
  searchCandidatesRef.current = React.useMemo(() => {
    const keys = new Set<string>();
    const teams = new Set<string>();
    for (const record of records) {
      if (record.scope?.scope === "identity" && record.scope.identity) keys.add(record.scope.identity);
      if (record.scope?.scope === "mob" && record.scope.mob) teams.add(record.scope.mob);
    }
    for (const scope of overview?.scopes ?? []) {
      if (scope.scope_kind === "identity" && scope.scope_key) keys.add(scope.scope_key);
      if (scope.scope_kind === "mob" && scope.scope_key) teams.add(scope.scope_key);
    }
    return [
      ...Array.from(keys, (key) => ({ key, name: agentDisplayName(key, searchDirectory) })),
      ...Array.from(teams, (key) => ({ key, name: teamName(key) })),
    ];
  }, [records, overview, searchDirectory]);
  const pagerRef = React.useRef<ReturnType<typeof createMemoryRecordsPager> | null>(null);
  if (!pagerRef.current) {
    pagerRef.current = createMemoryRecordsPager({
      query: (params) => (queryRecordsRef.current ? queryRecordsRef.current(params) : Promise.resolve(null)),
      setPaged,
      setLoading: setPageLoading,
      resolveKey: (text) => resolveRecordsSearchKey(text, searchCandidatesRef.current),
    });
  }
  const pager = pagerRef.current;
  const [lattice, setLattice] = React.useState<LatticeWalkResult | null>(null);
  const [latticeRunning, setLatticeRunning] = React.useState(false);
  const [knowledgeIdentity, setKnowledgeIdentity] = React.useState<string>("");

  // Lattice invariants (a) and (c) via the client page-walk. Deduped by a
  // CONTENT fingerprint (not array identity): content-identical debounced
  // live refreshes skip the walk entirely. While a re-check runs, the prior
  // result is retained so a settled tile never flickers to UNVERIFIABLE.
  // Nothing walks before the first records page lands: the panel can render
  // first with an empty placeholder, and a verdict settled over it would
  // make the first page's arrival walk the same store a second time, in the
  // middle of whatever the operator does next. The walk's records also draw
  // the landscape's ticks and widen the Records search.
  const latticeRanForRef = React.useRef<string | null>(null);
  React.useEffect(() => {
    // A runtime without a memory store has nothing to walk.
    if (tab !== "holdings" || !onQueryRecords || recordsDenied || !loaded || unavailable) return;
    const fingerprint = latticeFingerprint(records, realms, nextCursor);
    if (latticeRanForRef.current === fingerprint) return;
    latticeRanForRef.current = fingerprint;
    setLatticeRunning(true);
    let cancelled = false;
    let covered = false;
    void runLatticeWalk((params) => onQueryRecords(params), {
      realms,
      isCancelled: () => cancelled,
    })
      .then((result) => {
        // null means denied or cancelled; keep the prior verdict on cancel,
        // clear it on a real walk that came back denied.
        if (cancelled) return;
        // Only a settled verdict covers this content; a denied walk walks
        // again with the next refresh, as a failed one does.
        covered = result !== null;
        setLattice(result);
      })
      .catch(() => {
        if (!cancelled) setLattice(null);
      })
      .finally(() => {
        if (!cancelled) setLatticeRunning(false);
      });
    return () => {
      cancelled = true;
      // A walk that did not settle a verdict never covered this fingerprint,
      // so un-mark it and the dep change that cancelled us starts a fresh walk
      // instead of skipping. Without this, a refresh landing mid-walk
      // (re-dock, the 250ms memory.* debounce) leaves the tile checking
      // forever: the fingerprint reads as already-checked while lattice is
      // still null and latticeRunning was never cleared (its finally is
      // cancellation-guarded). A covered fingerprint stays marked, so a
      // refresh that brings the same records does not walk them again.
      if (!covered && latticeRanForRef.current === fingerprint) {
        latticeRanForRef.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab, records, recordsDenied, realms, nextCursor, loaded, unavailable]);

  // When the data last landed: each good refresh delivers new record arrays,
  // a failed one keeps the old ones (and shows its error instead).
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const updatedAt = React.useMemo(() => (loaded ? Date.now() : null), [records, loaded]);

  // Overview scopes with denied-probe kinds dropped (their denied rows render
  // instead); feeds the Holdings scopes, the totals and the store floor tile.
  const overviewScopes = React.useMemo(
    () =>
      overview
        ? sortOverviewScopes(visibleOverviewScopes(overview.scopes || [], { operatorScopeDenied, mobScopeDenied }))
        : null,
    [overview, operatorScopeDenied, mobScopeDenied],
  );
  const tiles = React.useMemo(
    () =>
      computeVerdictTiles({
        records,
        recordsDenied,
        dreams,
        dreamsDenied,
        dreamsWithheld,
        lattice,
        latticeRunning,
        overview: overviewScopes && overview ? { scopes: overviewScopes, floors: overview.floors } : null,
        overviewDenied,
        scopeName: (scope) => overviewScopeName(scope, nameOf),
      }),
    [records, recordsDenied, dreams, dreamsDenied, dreamsWithheld, lattice, latticeRunning, overview, overviewScopes, overviewDenied, nameOf],
  );
  // Every record the console has read: the base page, the paged queries,
  // the trust rules walk and the quarantine queue.
  const readRecords = React.useMemo(
    () => mergeReadRecords(records, paged?.records, lattice?.records, quarantineRecords),
    [records, paged, lattice, quarantineRecords],
  );
  // Record titles for every place that only holds an ID (violations,
  // injections, review verdicts, dream samples, promotions).
  const recordTitles = React.useMemo(() => {
    const titles = new Map<string, string>();
    const add = (list: ReadonlyArray<Pick<MemoryPanelRecord, "id" | "title">> | undefined) => {
      for (const entry of list ?? []) {
        if (entry.title && !titles.has(entry.id)) titles.set(entry.id, entry.title);
      }
    };
    add(readRecords);
    if (detail) add([detail.record, ...detail.chain]);
    return titles;
  }, [readRecords, detail]);
  const annotatedInjections = React.useMemo(() => annotateInjectionDups(injections), [injections]);
  const dreamSheets = React.useMemo(() => dreamRunsNewestFirst(dreamRuns), [dreamRuns]);
  const sheetPartitions = React.useMemo(() => new Map(dreamRuns.map((run) => [run.run_id, run.partition])), [dreamRuns]);
  const dedupedVerdicts = React.useMemo(() => dedupeAuditVerdicts(auditVerdicts), [auditVerdicts]);
  const identities = React.useMemo(() => identityOptions(records), [records]);
  const selectedIdentity = identities.includes(knowledgeIdentity) ? knowledgeIdentity : identities[0] || "";
  const memoryFrames = React.useMemo(() => liveFrames.filter((frame) => frame.event.startsWith("memory.")), [liveFrames]);
  const totals = React.useMemo(
    () => (overviewScopes ? storeTotals(overviewScopes) : loadedTotals(records)),
    [overviewScopes, records],
  );
  const multiRealm = realms.length > 1;

  function applyFilter(next: MemoryRecordsFilter): void {
    setFilter(next);
    if (!onQueryRecords) return;
    void pager.applyFilter(next);
  }

  function loadMore(): void {
    if (!onQueryRecords) return;
    void pager.loadMore({ filter, paged, baseRecords: records, baseCursor: nextCursor });
  }

  function openRecordsFiltered(next: MemoryRecordsFilter): void {
    setTab("records");
    onClearDetail();
    applyFilter(next);
  }

  function openTile(tile: VerdictTile): void {
    if (tile.id === "recall") setSortMode("utility");
    setTab(tile.targetTab);
  }

  function closeDetail(): void {
    setDecisionNotice(null);
    onClearDetail();
  }

  function selectRecord(realm: string | undefined, memoryId: string): void {
    setDecisionNotice(null);
    onSelectRecord(realm, memoryId);
  }

  if (unavailable) return <MemoryNotSetUp runtimeControl={runtimeControl} />;

  const listView = buildRecordsListView({ records, paged, baseCursor: nextCursor, filter, sortMode });
  const quarantineCount = quarantineRecords.length + pendingPromotions.length;
  const tabs: Array<PanelTab<MemoryTab>> = MEMORY_TABS.map((candidate) => ({
    id: candidate,
    label: memoryTabLabel(candidate),
    count:
      candidate === "pipeline" && quarantineCount > 0
        ? quarantineCount
        : candidate === "dreams" && dreams.length > 0
          ? dreams.length
          : null,
    testId: `memory-tab:${candidate}`,
  }));
  const glanceInputs: GlanceInputs = {
    loaded,
    recordsDenied,
    totals,
    totalsFromStore: overviewScopes !== null,
    morePages: Boolean(nextCursor),
    multiRealm,
    realmCount: realms.length,
    canReviewQuarantine,
    quarantined: quarantineRecords.length,
    waiting: pendingPromotions.length,
    injections,
    injectionsDenied,
    readRecords,
    tiles,
    now,
  };
  const refresh = () =>
    onRefresh(tab === "records" && detail ? { realm: detail.realm, memoryId: detail.record.id } : undefined);

  function renderHoldings(): React.JSX.Element {
    const rows = holdingsRows(overviewScopes, records);
    return (
      <div className="memory-tab memory-holdings" data-testid="memory-holdings">
        <ChecksStrip tiles={tiles} recordTitles={recordTitles} onOpen={openTile} onOpenRecord={selectRecord} />
        {recordsDenied ? (
          <HoldingsDenied />
        ) : !loaded ? (
          <HoldingsLoading />
        ) : (
          <Landscape
            rows={rows}
            records={readRecords}
            readComplete={Boolean(lattice?.complete) || (!nextCursor && !lattice)}
            readCount={readRecords.length}
            overviewLoaded={overviewScopes !== null}
            multiRealm={multiRealm}
            floors={overview?.floors}
            operatorScopeDenied={operatorScopeDenied}
            mobScopeDenied={mobScopeDenied}
            now={now}
            onOpenScope={openRecordsFiltered}
            onOpenRecord={selectRecord}
          />
        )}
        <InProgress
          dreams={dreams}
          dreamsDenied={dreamsDenied}
          dreamsWithheld={dreamsWithheld}
          canReviewQuarantine={canReviewQuarantine}
          quarantineRecords={quarantineRecords}
          pendingPromotions={pendingPromotions}
          proposals={proposals}
          proposalsDenied={proposalsDenied}
          onOpenPipeline={() => setTab("pipeline")}
          onOpenDreams={() => setTab("dreams")}
        />
        <Harvests harvests={harvests} denied={harvestsDenied} />
      </div>
    );
  }

  function pipelineFlow(): Array<{ key: string; label: string; stages: StageModel[]; split?: boolean }> {
    const decisions = dreamDecisions(dreamSheets.slice(0, 1));
    const lastDream = dreamSheets[0];
    const lastDreamAt = lastDream?.completed_at_ms || lastDream?.started_at_ms || dreams[0]?.last_op_at_ms;
    const released = readRecords.filter((record) => record.ever_quarantined === true && record.status?.status === "active").length;
    const review = (value: number): number | "no access" => (canReviewQuarantine ? value : "no access");
    return [
      {
        key: "new",
        label: "New memory",
        stages: [
          { key: "proposed", mark: "proposed", label: "Proposed", value: proposalsDenied ? "no access" : proposals.length },
          {
            key: "dream",
            mark: "dream",
            label: "Dream review",
            value: null,
            note: lastDreamAt
              ? `Last run ${formatRelativeTime(lastDreamAt, now)}${
                  decisions.accepted + decisions.held > 0 ? `: ${decisions.accepted} kept, ${decisions.held} held` : ""
                }`
              : dreamsDenied
                ? "no access"
                : dreamsWithheld || dreamRunsWithheld
                  ? "withheld by access"
                  : "no dream yet",
            onOpen: () => setTab("dreams"),
          },
          { key: "active", mark: "active", label: "Active", value: recordsDenied ? "no access" : totals.active, onOpen: () => setTab("records") },
        ],
      },
      {
        key: "quarantine",
        label: "Quarantine",
        split: true,
        stages: [
          { key: "quarantined", mark: "quarantined", label: "Quarantined", value: review(quarantineRecords.length) },
          { key: "released", mark: "released", label: "Released", value: recordsDenied ? "no access" : released },
          { key: "discarded", mark: "discarded", label: "Discarded", value: recordsDenied ? "no access" : totals.tombstoned },
        ],
      },
      {
        key: "team",
        label: "Team memory",
        stages: [
          { key: "waiting", mark: "waiting", label: "Waiting for approval", value: review(pendingPromotions.length) },
          { key: "team", mark: "team", label: "Team memory", value: recordsDenied ? "no access" : totals.team },
        ],
      },
    ];
  }

  const main = (
    <PanelBody className="memory-panel__body" tabPanel={{ idBase: tabIds, tab }}>
      {tab === "holdings" ? renderHoldings() : null}
      {tab === "records" ? (
        <RecordsTab
          listView={listView}
          readRecords={readRecords}
          filter={filter}
          onFilterEdit={setFilter}
          onFilterApply={applyFilter}
          onFilterBlur={() => {
            // Re-query only on a real change: a blur caused by clicking
            // load more must not race the append.
            void pager.applyFilterIfChanged(filter);
          }}
          sortMode={sortMode}
          onSortMode={setSortMode}
          view={recordsView}
          onView={setRecordsView}
          refinement={refinement}
          onRefinement={setRefinement}
          realms={realms}
          loaded={loaded}
          pageLoading={pageLoading}
          recordsDenied={recordsDenied}
          canQuery={Boolean(onQueryRecords)}
          detail={detail}
          now={now}
          onSelectRecord={selectRecord}
          onLoadMore={loadMore}
        />
      ) : null}
      {tab === "knowledge" ? (
        <KnowledgeTab
          records={records}
          identities={identities}
          selectedIdentity={selectedIdentity}
          onSelectIdentity={setKnowledgeIdentity}
          annotatedInjections={annotatedInjections}
          injectionsDenied={injectionsDenied}
          recordsDenied={recordsDenied}
          recordTitles={recordTitles}
          now={now}
          onOpenRecordsFiltered={openRecordsFiltered}
          onSelectRecord={selectRecord}
        />
      ) : null}
      {tab === "pipeline" ? (
        <PipelineTab
          flow={pipelineFlow()}
          proposals={proposals}
          proposalsDenied={proposalsDenied}
          pendingPromotions={pendingPromotions}
          quarantineRecords={quarantineRecords}
          canReviewQuarantine={canReviewQuarantine}
          canDecideQuarantine={Boolean(onDecideQuarantine)}
          auditVerdicts={dedupedVerdicts}
          auditVerdictsDenied={auditVerdictsDenied}
          recordTitles={recordTitles}
          memoryFrames={memoryFrames}
          onSelectRecord={selectRecord}
          onOpenGating={onOpenGating}
        />
      ) : null}
      {tab === "dreams" ? (
        <DreamsTab
          dreams={dreams}
          dreamsDenied={dreamsDenied}
          dreamSheets={dreamSheets}
          dreamRunsDenied={dreamRunsDenied}
          dreamsWithheld={dreamsWithheld}
          dreamRunsWithheld={dreamRunsWithheld}
          sheetPartitions={sheetPartitions}
          recordTitles={recordTitles}
          multiRealm={multiRealm}
          now={now}
          onSelectRecord={selectRecord}
        />
      ) : null}
    </PanelBody>
  );

  const drawer = detail ? (
    <Biography
      detail={detail}
      dreams={dreams}
      auditVerdicts={auditVerdicts}
      recordTitles={recordTitles}
      onClose={closeDetail}
      onSelectRecord={selectRecord}
      onLoadEvidence={onLoadEvidence}
      onDecideQuarantine={decideQuarantine}
      decisionNotice={decisionNotice}
    />
  ) : detailLoading ? (
    <div className="memory-drawer-loading">
      <LoadingState label="Loading record" rows={5} />
    </div>
  ) : null;

  return (
    <PanelShell className="memory-panel" testId="memory-panel" density={density} showTechnicalIds={showTechnicalIds}>
      <PanelHeader
        title="Memory"
        scope={runtimeControl || undefined}
        summary={memorySummaryFacts(glanceInputs)}
        freshness={loaded && updatedAt ? { at: updatedAt } : undefined}
        divider={false}
        tools={<IconButton label="Refresh memory" glyph="refresh" data-testid="memory-refresh" onClick={refresh} />}
        menu={{
          label: "More actions for Memory",
          triggerTestId: "memory-menu",
          items: standardPanelMenu({
            density,
            onDensityChange: setDensity,
            showTechnicalIds,
            onShowTechnicalIdsChange: setShowTechnicalIds,
            getCopyData: () => ({ records, realms, overview, quarantineRecords, pendingPromotions, dreams, dreamRuns, proposals, injections, harvests, auditVerdicts, detail }),
            onRefresh: refresh,
            testIdPrefix: "memory",
          }),
        }}
      />
      <MemoryGlance
        inputs={glanceInputs}
        onOpenRecords={() => setTab("records")}
        onOpenReview={() => setTab("pipeline")}
        onOpenGiven={() => setTab("knowledge")}
        onOpenChecks={() => setTab("holdings")}
      />
      <PanelTabs<MemoryTab> tabs={tabs} active={tab} onSelect={setTab} ariaLabel="Memory views" idBase={tabIds} />
      {/* One-release redirect alias (open Q6): the quarantine tab folded into
          Pipeline; automation addressing memory-tab:quarantine keeps working
          until the next release removes this. Outside the tab list, invisible
          and out of the tab order. */}
      <button
        type="button"
        className="memory-tab-alias"
        onClick={() => setTab(resolveMemoryTabAlias("quarantine") || "pipeline")}
        data-testid="memory-tab:quarantine"
        aria-hidden="true"
        tabIndex={-1}
      >
        Quarantine
      </button>
      {/* The status toast overlays the body: a refresh error or its recovery
          never moves the tabs or the body under a click. */}
      <StatusRow mode="toast" error={error} errorTestId="memory-error" updatedAt={updatedAt ?? undefined} onRetry={refresh} />
      <DrawerLayout
        className="memory-stage"
        main={main}
        drawer={drawer}
        open={Boolean(detail) || detailLoading}
        onClose={closeDetail}
        persistKey="memory"
        defaultWidth={400}
        backLabel="All records"
      />
    </PanelShell>
  );
}
