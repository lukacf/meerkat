import React from "react";
import { classifyFrame } from "../lib/frame-classifier";
import { agentHealth, isAgentWorking } from "../lib/agent-status";
import { useActivityLedger } from "../lib/activity-ledger";
import type { ConsoleAgent, ConsoleFrame } from "../types";
import {
  AgentAvatar,
  AgentName,
  Button,
  DetailDrawer,
  DrawerLayout,
  DrawerSection,
  EmptyState,
  FigureTile,
  FilterBar,
  GlanceStrip,
  IconButton,
  KitGlyph,
  LoadingState,
  Notice,
  PanelBody,
  PanelHeader,
  PanelShell,
  RelativeTime,
  StatusMark,
  StatusPill,
  agentDisplayName,
  formatAbsoluteTime,
  formatClockTime,
  formatDuration,
  plural,
  standardPanelMenu,
  useAgentDirectory,
  useConsoleNavigation,
  usePanelDensity,
  useShowTechnicalIds,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import { Swimlanes, type SwimlaneItem } from "../../../packages/console-components/src/viz/charts/swimlanes";
import {
  buildActivityEntries,
  entrySentence,
  isProblem,
  mergeFrameSources,
  type ActivityEntry,
} from "./activity/activity-model";
import {
  STORY_FILTERS,
  buildLaneModel,
  buildStory,
  filterCounts,
  foldFacts,
  inWindow,
  rowEntries,
  todayBins,
  todayWindow,
  turnFacts,
  turnOutcomeWords,
  type StoryFilter,
  type StoryFold,
  type StoryRow,
  type TodayView,
} from "./activity/story-model";
import { useHistoryBackfill } from "./activity/use-history-backfill";
import { useVirtualRows } from "./activity/use-virtual-rows";
import type { PendingApprovalLike } from "./activity/signals-model";

interface TimelinePanelProps {
  /** The console's live activity buffer, newest first. */
  frames: ConsoleFrame[];
  /** The agents' loaded history logs, in any order. */
  historyFrames?: readonly ConsoleFrame[];
  /** The roster: lane names, who is working now, and whose history to load. */
  agents?: readonly ConsoleAgent[];
  /** Approvals waiting for a decision. */
  approvals?: readonly PendingApprovalLike[];
  /** Loads one agent's history for the day (the console's identity log). */
  loadHistory?: (identity: string) => Promise<void>;
  /** Fallbacks while the console provides no navigation context. */
  onOpenAgent?: (identity: string, how: "details" | "chat") => void;
  onOpenApproval?: (pendingId: string) => void;
}

/** Part of a sentence: plain words, or an agent shown by display name
 * (null for events without one, which read "System"). */
type Segment = string | { agent: string | null };

/** One event of the day in words: a sentence, an optional excerpt, a tone
 * for its mark and its family. Interaction IDs never appear. */
export interface TodayLine {
  segments: Segment[];
  snippet?: string;
  tone: Tone;
  kind: string;
}

/** The sentence for one frame, from the shared classifier. Agents are named
 * through the directory when rendered; frames without an agent read
 * "System". */
export function describeTodayFrame(frame: ConsoleFrame): TodayLine {
  const classified = classifyFrame(frame);
  return {
    segments: classified.segments,
    ...(classified.snippet ? { snippet: classified.snippet } : {}),
    tone: classified.tone,
    kind: classified.severity === "crit" ? "Problem" : classified.familyWord,
  };
}

const VIEW_OPTIONS: ReadonlyArray<{ id: TodayView; label: string; testId: string }> = [
  { id: "day", label: "Day", testId: "timeline-view:day" },
  { id: "3h", label: "3h", testId: "timeline-view:3h" },
  { id: "1h", label: "1h", testId: "timeline-view:1h" },
];
const ROW_ESTIMATE = { comfortable: 46, compact: 34 } as const;
const HOUR_ESTIMATE = 30;

type StoryItem =
  | { kind: "hour"; key: string; startMs: number; count: number }
  | { kind: "row"; key: string; row: StoryRow; depth: 0 | 1 };

type Selection =
  | { kind: "row"; key: string }
  | { kind: "span"; id: string; agent: string; interactionId?: string; runId?: string }
  | { kind: "entry"; key: string };

function severityMarkTone(row: StoryRow): Tone {
  if (row.severity === "crit") return "crit";
  if (row.severity === "warn") return "warn";
  if (row.kind === "event") {
    if (row.entry.family === "approval") return "warn";
    return row.entry.tone === "ok" ? "ok" : row.entry.tone === "info" ? "info" : "neutral";
  }
  if (row.kind === "turn") {
    const outcome = turnOutcomeWords(row);
    return outcome?.tone === "ok" ? "ok" : "neutral";
  }
  return "neutral";
}

function storyRowTestKey(row: StoryRow): string {
  return row.key;
}

function joinWords(parts: ReadonlyArray<string | null | undefined | false>): string {
  return parts.filter(Boolean).join(" · ");
}

/** The sentence with agents as names (links when the console can open them). */
function Sentence({ entry, link }: { entry: Pick<ActivityEntry, "segments">; link: boolean }) {
  return (
    <>
      {entry.segments.map((segment, index) =>
        typeof segment === "string" ? (
          <React.Fragment key={index}>{segment}</React.Fragment>
        ) : segment.agent === null ? (
          <span key={index} className="today__who">System</span>
        ) : (
          <AgentName key={index} id={segment.agent} link={link} className="today__who" />
        ),
      )}
    </>
  );
}

export function TimelinePanel({
  frames,
  historyFrames,
  agents,
  approvals,
  loadHistory,
  onOpenAgent,
  onOpenApproval,
}: TimelinePanelProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const navigation = useConsoleNavigation();
  const ledger = useActivityLedger({ tickMs: 30_000 });
  const [view, setView] = React.useState<TodayView>("day");
  const [filter, setFilter] = React.useState<StoryFilter>("all");
  const [query, setQuery] = React.useState("");
  const [density, setDensity] = usePanelDensity("timeline");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("timeline");
  const [selection, setSelection] = React.useState<Selection | null>(null);
  const [open, setOpen] = React.useState<Set<string>>(() => new Set());
  const storyRef = React.useRef<HTMLDivElement | null>(null);
  const scrollRef = React.useRef<HTMLElement | null>(null);
  const nameOf = React.useCallback((key: string) => agentDisplayName(key, directory) || key, [directory]);
  const canLink = Boolean(navigation?.openAgent);
  const nowMs = ledger.nowMs;

  // Load the day's history of the most active agents, four at a time.
  const history = useHistoryBackfill(agents, loadHistory);
  const loading = history.loading;
  const loadedOnce = history.loaded;

  const merged = React.useMemo(() => mergeFrameSources(frames, historyFrames), [frames, historyFrames]);
  const allEntries = React.useMemo(() => {
    const entries = buildActivityEntries(merged, { directory });
    const approvalEntries = (approvals ?? []).flatMap((approval) => approvalEntry(approval, nowMs));
    return approvalEntries.length ? [...approvalEntries, ...entries].sort((a, b) => b.atMs - a.atMs) : entries;
  }, [merged, directory, approvals, nowMs]);
  // The day ends now, or at its newest event when a clock runs ahead.
  const newestMs = allEntries[0]?.atMs;
  const range = React.useMemo(() => {
    const base = todayWindow(view, nowMs);
    if (view === "day" && newestMs !== undefined && newestMs > base.end && newestMs < base.start + 24 * 3_600_000) {
      return { start: base.start, end: newestMs };
    }
    return base;
  }, [view, nowMs, newestMs]);
  const entries = React.useMemo(() => allEntries.filter((entry) => inWindow(entry.atMs, range)), [allEntries, range]);
  const updatedAt = React.useRef<{ entries: ActivityEntry[]; at: number } | null>(null);
  if (updatedAt.current?.entries !== allEntries) updatedAt.current = { entries: allEntries, at: Date.now() };

  const runs = React.useMemo(() => ledger.runSpans(null, { sinceMs: range.start, untilMs: range.end }), [ledger, range]);
  const tools = React.useMemo(() => ledger.toolSpans(null, { sinceMs: range.start, untilMs: range.end }), [ledger, range]);
  const agentsByKey = React.useMemo(() => {
    const map = new Map<string, ConsoleAgent>();
    for (const agent of agents ?? []) {
      for (const key of [agent.identity, agent.member_id]) if (key) map.set(key, agent);
    }
    return map;
  }, [agents]);
  const isLive = React.useCallback((identity: string) => {
    const known = directory.lookup(identity);
    const agent = agentsByKey.get(identity) ?? (known ? agentsByKey.get(known.id) : undefined);
    const openRun = ledger.hasOpenRun(identity);
    return agent ? isAgentWorking(agent, { openRun }) : openRun;
  }, [agentsByKey, directory, ledger]);
  const lanes = React.useMemo(
    () => buildLaneModel({ entries, runs, tools, window: range, nameOf, isLive }),
    [entries, runs, tools, range, nameOf, isLive],
  );

  // Glance figures: the ledger counts turns, tools and messages (once per
  // message); the entries count problems.
  const totals = React.useMemo(() => ledger.totals(null, { sinceMs: range.start, untilMs: range.end }), [ledger, range]);
  const bins = todayBins(view, range);
  const turnTrend = React.useMemo(
    () => ledger.series(null, "turnsStarted", { bins: Math.min(bins.bins, 48), binMs: bins.binMs, endMs: range.end }),
    [ledger, bins.bins, bins.binMs, range.end],
  );
  const traffic = React.useMemo(() => ledger.traffic({ sinceMs: range.start, untilMs: range.end }), [ledger, range]);
  const messageCount = traffic.reduce((sum, pair) => sum + pair.count, 0);
  const crit = entries.filter((entry) => entry.severity === "crit");
  const warn = entries.filter((entry) => entry.severity === "warn");
  const failedTurns = crit.filter((entry) => entry.family === "turn").length;
  const activeAgents = new Set(entries.map((entry) => entry.actor).filter(Boolean)).size;
  const workingNow = lanes.lanes.filter((lane) => lane.live).length;
  const toolProblems = entries.filter((entry) => (entry.family === "tool" || entry.family === "workgraph") && isProblem(entry)).length;
  const counts = React.useMemo(() => filterCounts(entries), [entries]);

  const story = React.useMemo(() => buildStory(entries, { filter, query, nameOf }), [entries, filter, query, nameOf]);
  const items = React.useMemo<StoryItem[]>(() => {
    const list: StoryItem[] = [];
    for (const hour of story) {
      list.push({ kind: "hour", key: hour.key, startMs: hour.startMs, count: hour.count });
      for (const row of hour.rows) {
        list.push({ kind: "row", key: row.key, row, depth: 0 });
        if (row.kind === "fold" && open.has(row.key)) {
          for (const child of row.rows) list.push({ kind: "row", key: `${row.key}/${child.key}`, row: child, depth: 1 });
        }
      }
    }
    return list;
  }, [story, open]);

  React.useLayoutEffect(() => {
    scrollRef.current = storyRef.current?.closest<HTMLElement>(".cc-drawer-layout__main") ?? null;
  });
  const virtual = useVirtualRows({
    count: items.length,
    keyAt: (index) => items[index].key,
    estimate: (index) => (items[index].kind === "hour" ? HOUR_ESTIMATE : ROW_ESTIMATE[density]),
    scrollRef,
    listRef: storyRef,
  });

  const openChat = (identity: string) => {
    if (navigation?.openAgent) navigation.openAgent(identity, "chat");
    else onOpenAgent?.(identity, "chat");
  };
  const openDetails = (identity: string) => {
    if (navigation?.openAgent) navigation.openAgent(identity, "details");
    else onOpenAgent?.(identity, "details");
  };
  const openApproval = (pendingId: string) => {
    if (navigation?.openApproval) navigation.openApproval(pendingId);
    else onOpenApproval?.(pendingId);
  };
  const canOpenAgent = Boolean(navigation?.openAgent || onOpenAgent);
  const canOpenApproval = Boolean(navigation?.openApproval || onOpenApproval);

  const rowByKey = React.useMemo(() => {
    const map = new Map<string, StoryRow>();
    for (const hour of story) {
      for (const row of hour.rows) {
        map.set(row.key, row);
        if (row.kind === "fold") for (const child of row.rows) map.set(child.key, child);
      }
    }
    return map;
  }, [story]);
  const entryByKey = React.useMemo(() => new Map(allEntries.map((entry) => [entry.key, entry])), [allEntries]);

  const onLaneSelect = (item: SwimlaneItem) => {
    if (item.kind === "span") {
      const span = lanes.spans.find((candidate) => candidate.id === item.id);
      if (span) setSelection({ kind: "span", id: span.id, agent: span.agent, interactionId: span.interactionId, runId: span.runId });
      return;
    }
    const key = item.kind === "link"
      ? lanes.links.find((link) => link.id === item.id)?.entryKey
      : [...lanes.marks, ...lanes.problems].find((mark) => mark.id === item.id)?.entryKey;
    if (key) setSelection({ kind: "entry", key });
  };

  const selectedRow: StoryRow | undefined = (() => {
    if (!selection) return undefined;
    if (selection.kind === "row") return rowByKey.get(selection.key);
    if (selection.kind === "span") {
      const turn = buildStory(entries.filter((entry) => entry.actor === selection.agent && (!selection.interactionId || entry.interactionId === selection.interactionId)), { nameOf })
        .flatMap((hour) => hour.rows)
        .flatMap((row) => (row.kind === "fold" ? row.rows : [row]))
        .find((row) => row.kind === "turn" && (!selection.interactionId || row.interactionId === selection.interactionId));
      return turn;
    }
    const entry = entryByKey.get(selection.key);
    return entry ? { kind: "event", key: `event:${entry.key}`, entry, actor: entry.actor, atMs: entry.atMs, severity: entry.severity } : undefined;
  })();
  const selectedSpan = selection?.kind === "span" ? lanes.spans.find((span) => span.id === selection.id) : undefined;
  // The lane mark of what the drawer shows: a span, or an event's mark.
  const selectedLaneId = (() => {
    if (!selection) return null;
    if (selection.kind === "span") return selection.id;
    const markOf = (key: string) => [...lanes.problems, ...lanes.marks, ...lanes.links].find((mark) => mark.entryKey === key)?.id ?? null;
    if (selection.kind === "entry") return markOf(selection.key);
    const row = selectedRow;
    if (row?.kind === "event") return markOf(row.entry.key);
    if (row?.kind === "turn") {
      return lanes.spans.find((span) => span.agent === row.actor && span.interactionId === row.interactionId)?.id ?? null;
    }
    return null;
  })();

  const menuItems = standardPanelMenu({
    density,
    onDensityChange: setDensity,
    showTechnicalIds,
    onShowTechnicalIdsChange: setShowTechnicalIds,
    getCopyData: () => entries.slice(0, 500).map((entry) => ({
      at: new Date(entry.atMs).toISOString(),
      family: entry.family,
      severity: entry.severity,
      sentence: entrySentence(entry, nameOf),
      snippet: entry.snippet ?? null,
      agent: entry.actor,
      target: entry.target ?? null,
      interaction_id: entry.interactionId ?? null,
    })),
    onRefresh: loadHistory ? history.refresh : undefined,
    testIdPrefix: "timeline",
  });

  const windowWords = view === "day" ? "today" : view === "3h" ? "in the last 3 hours" : "in the last hour";
  const summary = [
    { key: "events", text: plural(entries.length, "event") + (view === "day" ? "" : view === "3h" ? " in 3h" : " in 1h") },
    { key: "agents", text: `${plural(activeAgents, "agent")} active` },
    workingNow ? { key: "working", text: `${workingNow} working now`, tone: "live" as const, announce: false } : null,
    failedTurns ? { key: "failed", text: plural(failedTurns, "failed turn"), tone: "crit" as const } : null,
  ];

  const oldestLoaded = allEntries.length ? allEntries[allEntries.length - 1].atMs : null;
  const partial = Boolean(oldestLoaded && oldestLoaded > range.start + 5 * 60_000 && !loadHistory);
  const firstLoad = loading && !loadedOnce && entries.length === 0;

  const toggleOpen = (key: string) => setOpen((prev) => {
    const next = new Set(prev);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    return next;
  });

  const renderStoryRow = (item: Extract<StoryItem, { kind: "row" }>) => {
    const { row, depth } = item;
    const selected = selection?.kind === "row" && selection.key === row.key;
    const markTone = severityMarkTone(row);
    const time = row.kind === "turn" ? row.startMs : row.atMs;
    let main: React.ReactNode;
    let meta: React.ReactNode = null;
    let actorKey: string | null = null;
    if (row.kind === "event") {
      const entry = row.entry;
      actorKey = entry.actor;
      main = (
        <>
          <Sentence entry={entry} link={canLink} />
          {entry.snippet ? <span className="today__snippet">{`: ${entry.snippet}`}</span> : null}
        </>
      );
      meta = joinWords([entry.family === "approval" && entry.frame.event === "approval_pending" ? "waiting for a decision" : null, entry.familyWord]);
    } else if (row.kind === "turn") {
      actorKey = row.actor;
      const lead = row.request ?? row.outcome ?? row.entries[0];
      const outcome = turnOutcomeWords(row);
      const working = !row.outcome && isLive(row.actor);
      main = (
        <>
          <Sentence entry={lead} link={canLink} />
          {lead.snippet ? <span className="today__snippet">{`: ${lead.snippet}`}</span> : null}
        </>
      );
      meta = (
        <>
          {working ? <span className="today__live"><StatusMark tone="live" />working now</span> : outcome ? (
            <span className="today__outcome" data-tone={outcome.tone}>{outcome.text}</span>
          ) : null}
          {turnFacts(row, nameOf).map((fact) => <span key={fact} className="today__fact">{fact}</span>)}
          {row.outcome && row.outcome.snippet && row.request ? <span className="today__result" title={row.outcome.snippet}>{row.outcome.snippet}</span> : null}
        </>
      );
    } else {
      actorKey = row.actor;
      main = (
        <>
          <AgentName id={row.actor} link={canLink} className="today__who" />
          <span className="today__fold-facts">{` · ${foldFacts(row, nameOf).join(" · ")}`}</span>
        </>
      );
    }
    const isFold = row.kind === "fold";
    const expanded = isFold && open.has(row.key);
    return (
      <div
        key={item.key}
        ref={virtual.measureRef(item.key)}
        className={[
          "today__row",
          `today__row--${row.kind}`,
          depth ? "today__row--nested" : "",
          selected ? "is-selected" : "",
          row.severity !== "info" ? `today__row--${row.severity}` : "",
          "cc-reveal-host",
        ].filter(Boolean).join(" ")}
        data-testid={`timeline-story-row:${storyRowTestKey(row)}`}
        data-severity={row.severity}
        role="listitem"
      >
        <time className="today__time" dateTime={new Date(time).toISOString()} title={formatAbsoluteTime(time)}>
          {formatClockTime(time)}
        </time>
        <span className="today__rail">
          <StatusMark tone={markTone} label={row.severity === "crit" ? "Failed" : row.severity === "warn" ? "Needs attention" : undefined} />
        </span>
        <span className="today__content">
          <span className="today__lead">
            {actorKey ? <AgentAvatar name={nameOf(actorKey)} size={18} live={row.kind === "turn" && !row.outcome && isLive(row.actor)} /> : null}
            <span className="today__text">{main}</span>
          </span>
          {meta ? <span className="today__meta">{meta}</span> : null}
        </span>
        <span className="today__actions">
          {isFold ? (
            <IconButton
              size="sm"
              glyph={expanded ? "collapse" : "expand"}
              label={expanded ? "Fold these events" : "Show these events"}
              aria-expanded={expanded}
              onClick={() => toggleOpen(row.key)}
              data-testid={`timeline-story-fold:${row.key}`}
            />
          ) : null}
          {row.kind !== "fold" && row.severity === "crit" && actorKey && canOpenAgent ? (
            <Button size="sm" variant="ghost" className="today__open-chat" onClick={() => openChat(actorKey!)}>
              Open chat
            </Button>
          ) : null}
          {row.kind === "event" && row.entry.frame.event === "approval_pending" && canOpenApproval ? (
            <Button size="sm" className="today__review" onClick={() => openApproval(String((row.entry.frame.data as { pending_id?: string })?.pending_id ?? ""))}>
              Review
            </Button>
          ) : null}
          <IconButton
            reveal
            quiet
            size="sm"
            glyph="arrow-right"
            label="Show details"
            className="today__details"
            aria-pressed={selected}
            onClick={() => setSelection(selected ? null : { kind: "row", key: row.key })}
            data-testid={`timeline-story-open:${row.key}`}
          />
        </span>
      </div>
    );
  };

  const drawer = selection ? (
    <TodayDrawer
      row={selectedRow}
      span={selectedSpan}
      nameOf={nameOf}
      canLink={canLink}
      agent={selectedRow?.actor ? agentsByKey.get(selectedRow.actor) : selectedSpan ? agentsByKey.get(selectedSpan.agent) : undefined}
      live={selectedRow?.actor ? isLive(selectedRow.actor) : selectedSpan ? isLive(selectedSpan.agent) : false}
      onClose={() => setSelection(null)}
      onOpenChat={canOpenAgent ? openChat : undefined}
      onOpenDetails={canOpenAgent ? openDetails : undefined}
      onOpenApproval={canOpenApproval ? openApproval : undefined}
    />
  ) : null;

  const main = (
    <div className="today__main">
      <section className="today__lanes" aria-label="Day lanes">
        {lanes.lanes.length ? (
          <Swimlanes
            lanes={lanes.lanes.map((lane) => ({
              id: lane.id,
              label: lane.label,
              title: lane.agents ? `${lane.label}: ${lane.agents.map(nameOf).join(", ")}` : lane.label,
              live: lane.live,
              folded: Boolean(lane.agents),
              ...(lane.agent && canOpenAgent ? { onActivate: () => openDetails(lane.agent!), activateLabel: `Open ${lane.label}'s details` } : {}),
            }))}
            spans={lanes.spans}
            links={lanes.links}
            marks={lanes.marks}
            problems={lanes.problems}
            start={range.start}
            end={range.end}
            now={nowMs}
            laneHeight={density === "compact" ? 22 : 28}
            label={`${plural(lanes.lanes.length, "lane")}, ${plural(lanes.spans.length, "turn")}, ${plural(lanes.links.length, "message")} and ${plural(lanes.problems.length, "problem")} ${windowWords}`}
            onSelect={onLaneSelect}
            selectedId={selectedLaneId}
            testIdPrefix="timeline"
          />
        ) : firstLoad ? (
          <LoadingState variant="timeline" label="Loading today's activity" />
        ) : null}
      </section>
      {partial ? (
        <Notice tone="info" flush testId="timeline-partial">
          {`Showing activity since ${formatClockTime(oldestLoaded!)}, when this console connected. Earlier events load per agent.`}
        </Notice>
      ) : null}
      <FilterBar
        className="today__filters"
        ariaLabel="Filter today"
        search={{ label: "Search today", value: query, onChange: setQuery, placeholder: "Search today", testId: "timeline-search" }}
        chips={STORY_FILTERS.map((option) => ({
          label: option.label,
          count: counts[option.id],
          pressed: filter === option.id,
          tone: option.id === "problems" && counts.problems ? ("crit" as const) : undefined,
          onToggle: (on: boolean) => setFilter(on ? option.id : "all"),
          testId: `timeline-filter:${option.id}`,
        }))}
        modified={filter !== "all" || Boolean(query)}
        onClear={() => {
          setFilter("all");
          setQuery("");
        }}
      />
      <div className="today__story" ref={storyRef} role="list" aria-label="The day's events, newest first">
        {items.length ? (
          <>
            <div style={{ height: virtual.padTop }} aria-hidden="true" />
            {items.slice(virtual.start, virtual.end).map((item) =>
              item.kind === "hour" ? (
                <div key={item.key} ref={virtual.measureRef(item.key)} className="today__hour" role="presentation" data-testid={`timeline-hour:${item.startMs}`}>
                  <span className="today__hour-label">{formatClockTime(item.startMs)}</span>
                  <span className="today__hour-count">{plural(item.count, "event")}</span>
                </div>
              ) : renderStoryRow(item),
            )}
            <div style={{ height: virtual.padBottom }} aria-hidden="true" />
          </>
        ) : firstLoad ? (
          <LoadingState variant="rows" rows={6} label="Loading today's activity" />
        ) : entries.length ? (
          <EmptyState
            kind="filtered"
            illustration="search"
            inline
            title={query ? `Nothing ${windowWords} matches "${query}"` : `No ${STORY_FILTERS.find((option) => option.id === filter)?.label.toLowerCase()} ${windowWords}`}
            action={<Button size="sm" onClick={() => { setFilter("all"); setQuery(""); }}>Clear filters</Button>}
          />
        ) : (
          <EmptyState
            illustration="timeline"
            title={view === "day" ? "Nothing has happened yet today" : `Nothing has happened ${windowWords}`}
            body="Turns, approvals, messages and changes to the team appear here as they happen."
            testId="timeline-empty"
          />
        )}
      </div>
    </div>
  );

  return (
    <PanelShell className="today" testId="timeline-panel" density={density} showTechnicalIds={showTechnicalIds}>
      <PanelHeader
        title="Today"
        summary={summary}
        summaryTestId="timeline-summary"
        freshness={{ at: updatedAt.current.at, updating: loading }}
        refreshing={loading}
        view={{ ariaLabel: "Time span", options: VIEW_OPTIONS, value: view, onChange: setView, testId: "timeline-view" }}
        tools={loadHistory ? (
          <IconButton label="Refresh today" glyph="refresh" onClick={history.refresh} data-testid="timeline-refresh" />
        ) : undefined}
        menu={{ label: "More actions for Today", items: menuItems, triggerTestId: "timeline-menu" }}
      />
      <GlanceStrip ariaLabel="Today at a glance" testId="timeline-glance" className="today__glance">
        <FigureTile
          label="Turns"
          value={totals.turnsStarted}
          trend={turnTrend}
          trendLive={workingNow > 0}
          trendLabel={`Turns per ${bins.binMs >= 3_600_000 ? plural(bins.binMs / 3_600_000, "hour") : `${bins.binMs / 60_000} minutes`} ${windowWords}`}
          foot={workingNow ? `${workingNow} working now` : `${plural(totals.turnsFinished, "turn")} finished`}
          testId="timeline-glance:turns"
        />
        <FigureTile
          label="Messages between agents"
          value={messageCount}
          foot={traffic[0] ? `${nameOf(traffic[0].from)} to ${nameOf(traffic[0].to)}: ${traffic[0].count}` : "none yet"}
          testId="timeline-glance:messages"
        />
        <FigureTile
          label="Tools used"
          value={totals.toolCalls}
          foot={toolProblems ? plural(toolProblems, "tool problem") : "no tool problems"}
          testId="timeline-glance:tools"
        />
        <FigureTile
          label="Problems"
          tone={crit.length ? "crit" : warn.length ? "warn" : undefined}
          value={crit.length + warn.length}
          foot={crit.length + warn.length
            ? joinWords([crit.length ? plural(crit.length, "failure") : null, warn.length ? plural(warn.length, "warning") : null])
            : "nothing went wrong"}
          onActivate={() => setFilter(filter === "problems" ? "all" : "problems")}
          pressed={filter === "problems"}
          title="Show only problems"
          testId="timeline-glance:problems"
        />
      </GlanceStrip>
      <PanelBody className="today__body">
        <DrawerLayout
          main={main}
          drawer={drawer}
          open={Boolean(selection && drawer)}
          onClose={() => setSelection(null)}
          persistKey="timeline"
          backLabel="Today"
          testId="timeline-layout"
        />
      </PanelBody>
    </PanelShell>
  );
}

/** A pending approval as a Today event: the agent asked, a person decides. */
function approvalEntry(approval: PendingApprovalLike, nowMs: number): ActivityEntry[] {
  if (approval.status && approval.status !== "pending") return [];
  const atMs = approval.createdAtMs;
  if (!atMs) return [];
  const actor = approval.actorId?.trim() || approval.origin?.identity?.trim() || null;
  const frame: ConsoleFrame = {
    id: `approval:${approval.pendingId}`,
    event: "approval_pending",
    ...(actor ? { identity: actor } : {}),
    ...(approval.origin?.interactionId ? { interactionId: approval.origin.interactionId } : {}),
    timestampMs: atMs,
    data: { pending_id: approval.pendingId, action: approval.action, deadline_at_ms: approval.deadlineAtMs, risk_tier: approval.riskTier },
  };
  const overdue = approval.deadlineAtMs !== undefined && approval.deadlineAtMs < nowMs;
  const classified = classifyFrame(frame);
  return [{
    key: `approval:${approval.pendingId}`,
    rowKey: `approval:${approval.pendingId}:0`,
    frame,
    frames: [frame],
    atMs,
    classified,
    family: "approval",
    familyWord: "Approval",
    severity: "info",
    tone: overdue ? "crit" : "warn",
    title: "Approval requested",
    segments: [{ agent: actor }, " asked for approval"],
    ...(approval.action ? { snippet: approval.action } : {}),
    actor,
    ...(approval.origin?.interactionId ? { interactionId: approval.origin.interactionId } : {}),
  }];
}

function spanStateWords(state: string | undefined): { word: string; tone: Tone } {
  switch (state) {
    case "live": return { word: "Working", tone: "live" };
    case "failed": return { word: "Failed", tone: "crit" };
    case "cancelled": return { word: "Stopped", tone: "neutral" };
    default: return { word: "Finished", tone: "ok" };
  }
}

/** The day's detail drawer: a turn (its request, what it did, how it
 * ended), an event, or a folded run of events. */
function TodayDrawer({
  row,
  span,
  nameOf,
  canLink,
  agent,
  live,
  onClose,
  onOpenChat,
  onOpenDetails,
  onOpenApproval,
}: {
  row?: StoryRow;
  span?: { id: string; agent: string; start: number; end?: number; state: string; ticks: number[]; runId?: string; interactionId?: string };
  nameOf: (key: string) => string;
  canLink: boolean;
  agent?: ConsoleAgent;
  live: boolean;
  onClose: () => void;
  onOpenChat?: (identity: string) => void;
  onOpenDetails?: (identity: string) => void;
  onOpenApproval?: (pendingId: string) => void;
}) {
  const actor = row?.actor ?? span?.agent ?? null;
  const health = agent ? agentHealth(agent, { openRun: live }) : null;
  const actions = actor ? (
    <>
      {onOpenChat ? (
        <Button variant="primary" size="sm" onClick={() => onOpenChat(actor)}>
          <KitGlyph name="chat" />
          Open chat
        </Button>
      ) : null}
      {onOpenDetails ? <Button size="sm" onClick={() => onOpenDetails(actor)}>Details</Button> : null}
    </>
  ) : null;

  if (row?.kind === "turn" || (!row && span)) {
    const turn = row?.kind === "turn" ? row : undefined;
    const outcome = turn ? turnOutcomeWords(turn) : null;
    const status = span ? spanStateWords(span.state) : turn?.outcome
      ? turn.outcome.severity === "crit" ? { word: "Failed", tone: "crit" as Tone } : { word: outcome?.text === "replied" ? "Finished" : outcome?.text ?? "Finished", tone: (outcome?.tone ?? "ok") as Tone }
      : live ? { word: "Working", tone: "live" as Tone } : { word: "No reply yet", tone: "neutral" as Tone };
    const start = turn?.startMs ?? span?.start ?? 0;
    const end = turn?.outcome?.atMs ?? span?.end;
    const name = actor ? nameOf(actor) : "System";
    return (
      <DetailDrawer
        title={`${name}'s turn`}
        mark={actor ? <AgentAvatar name={name} size={28} live={status.tone === "live"} /> : undefined}
        status={<StatusPill tone={status.tone}>{status.word}</StatusPill>}
        subtitle={joinWords([
          `${formatClockTime(start)}${end ? `-${formatClockTime(end)}` : ""}`,
          end && start ? formatDuration(end - start) : status.tone === "live" ? "still running" : null,
          health && health.kind !== "healthy" && health.kind !== "working" ? health.word : null,
        ])}
        onClose={onClose}
        actions={actions}
        technical={[
          { label: "Interaction", value: turn?.interactionId ?? span?.interactionId },
          { label: "Run", value: turn?.outcome?.runId ?? turn?.entries.find((entry) => entry.runId)?.runId ?? span?.runId },
          { label: "Session", value: turn?.entries.find((entry) => entry.frame.sessionId)?.frame.sessionId },
          turn ? { label: "Events", value: turn.entries.length, plain: true } : null,
        ]}
        testId="timeline-drawer"
      >
        {turn?.request ? (
          <DrawerSection label="Request">
            <p className="today__drawer-text"><Sentence entry={turn.request} link={canLink} /></p>
            {turn.request.snippet ? <p className="today__drawer-quote">{turn.request.snippet}</p> : null}
          </DrawerSection>
        ) : null}
        {turn ? (
          <DrawerSection label="What happened" count={turn.entries.length}>
            <ol className="today__drawer-list">
              {turn.entries.map((entry) => (
                <li key={entry.key} className="today__drawer-item" data-severity={entry.severity}>
                  <time className="today__time" dateTime={new Date(entry.atMs).toISOString()}>{formatClockTime(entry.atMs)}</time>
                  <StatusMark tone={entry.severity === "crit" ? "crit" : entry.severity === "warn" ? "warn" : entry.tone === "ok" ? "ok" : "neutral"} />
                  <span className="today__drawer-words">
                    <Sentence entry={entry} link={canLink} />
                    {entry.snippet && entry !== turn.request ? <span className="today__snippet">{`: ${entry.snippet}`}</span> : null}
                  </span>
                </li>
              ))}
            </ol>
          </DrawerSection>
        ) : span ? (
          <DrawerSection label="What happened">
            <p className="today__drawer-text">
              {span.ticks.length ? plural(span.ticks.length, "tool call") : "No tool calls"} in this turn. Its events load with the agent's history.
            </p>
          </DrawerSection>
        ) : null}
      </DetailDrawer>
    );
  }

  if (row?.kind === "fold") {
    const fold: StoryFold = row;
    const name = nameOf(fold.actor);
    const entries = fold.rows.flatMap(rowEntries);
    return (
      <DetailDrawer
        title={name}
        mark={<AgentAvatar name={name} size={28} live={live} />}
        status={health ? <StatusPill tone={health.tone === "live" ? "live" : health.tone}>{health.word}</StatusPill> : undefined}
        subtitle={joinWords([`${formatClockTime(fold.startMs)}-${formatClockTime(fold.atMs)}`, foldFacts(fold, nameOf).join(" · ")])}
        onClose={onClose}
        actions={actions}
        testId="timeline-drawer"
      >
        <DrawerSection label="Events" count={entries.length}>
          <ol className="today__drawer-list">
            {entries.slice().reverse().map((entry) => (
              <li key={entry.key} className="today__drawer-item">
                <time className="today__time" dateTime={new Date(entry.atMs).toISOString()}>{formatClockTime(entry.atMs)}</time>
                <StatusMark tone={entry.tone === "ok" ? "ok" : "neutral"} />
                <span className="today__drawer-words">
                  <Sentence entry={entry} link={canLink} />
                  {entry.snippet ? <span className="today__snippet">{`: ${entry.snippet}`}</span> : null}
                </span>
              </li>
            ))}
          </ol>
        </DrawerSection>
      </DetailDrawer>
    );
  }

  if (row?.kind === "event") {
    const entry = row.entry;
    const pendingId = entry.frame.event === "approval_pending" ? String((entry.frame.data as { pending_id?: string })?.pending_id ?? "") : "";
    const tone: Tone = entry.severity === "crit" ? "crit" : entry.severity === "warn" ? "warn" : entry.tone;
    const word = entry.severity === "crit" ? "Failed" : entry.severity === "warn" ? "Needs attention" : pendingId ? "Waiting" : entry.familyWord;
    return (
      <DetailDrawer
        title={entry.title}
        mark={<StatusMark tone={tone} size="md" />}
        status={<StatusPill tone={tone}>{word}</StatusPill>}
        subtitle={<RelativeTime value={entry.atMs} prefix={`${formatClockTime(entry.atMs)} ·`} />}
        onClose={onClose}
        actions={(
          <>
            {pendingId && onOpenApproval ? (
              <Button variant="primary" size="sm" onClick={() => onOpenApproval(pendingId)}>Review</Button>
            ) : null}
            {actions}
          </>
        )}
        technical={[
          { label: "Event", value: entry.frame.event, plain: true },
          { label: "Frame", value: entry.frame.id },
          { label: "Interaction", value: entry.interactionId },
          { label: "Run", value: entry.runId },
          entry.frames.length > 1 ? { label: "Copies", value: entry.frames.length, plain: true } : null,
        ]}
        testId="timeline-drawer"
      >
        <DrawerSection label="What happened">
          <p className="today__drawer-text"><Sentence entry={entry} link={canLink} /></p>
          {entry.snippet ? <p className="today__drawer-quote">{entry.snippet}</p> : null}
        </DrawerSection>
        {entry.actor || entry.target ? (
          <DrawerSection label={entry.target ? "Between" : "Agent"}>
            <span className="today__drawer-agents">
              {entry.actor ? <AgentName id={entry.actor} avatar={22} link={canLink} /> : null}
              {entry.actor && entry.target ? <KitGlyph name="arrow-right" className="today__drawer-arrow" /> : null}
              {entry.target ? <AgentName id={entry.target} avatar={22} link={canLink} /> : null}
            </span>
          </DrawerSection>
        ) : null}
      </DetailDrawer>
    );
  }

  return (
    <DetailDrawer title="Not in view" onClose={onClose} testId="timeline-drawer">
      <p className="today__drawer-text">This event is outside the time span shown.</p>
    </DetailDrawer>
  );
}
