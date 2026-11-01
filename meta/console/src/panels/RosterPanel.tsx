import React from "react";
import type { ConsoleAgent } from "../types";
import { agentDisplayTitle, responsePhaseWords } from "../lib/agents";
import { restartOffered } from "../lib/side-chat";
import { AGENT_BUCKET_ORDER as BUCKET_ORDER, bucketOf, type AgentBucket } from "../lib/agent-buckets";
import { agentStateWord as stateWord, isAgentRetired as isRetired, isAgentWorking as isWorking, type AgentHealth } from "../lib/agent-status";
import { useActivityLedger, type ActivityLedgerReader } from "../lib/activity-ledger";
import {
  HOUR_MS,
  agentActivity,
  agentHealthOf,
  agentKey,
  agentNowLine,
  agentWorking,
  countWords,
  hourCeiling,
  hourLabel,
  localDayStart,
  seriesWords,
  type AgentActivity,
  type AgentNowLine,
} from "../lib/agent-activity";
import { useElementWidth } from "../lib/use-element-width";
import { LifecycleConfirm, type ConfirmedLifecycleAction } from "./LifecycleConfirm";
import {
  ActivityBars,
  AgentAvatar,
  AgentName,
  BarcodeLegend,
  BarcodeMeter,
  Button,
  Count,
  DetailDrawer,
  DrawerLayout,
  DrawerSection,
  EmptyState,
  EntityCard,
  EntityRow,
  FigureTile,
  FilterBar,
  GlanceStrip,
  HeatStrip,
  IconButton,
  KeyValueList,
  KitGlyph,
  PanelHeader,
  PanelShell,
  RelativeTime,
  SegmentedControl,
  StatusMark,
  StatusPill,
  humanizeEnum,
  joinMeta,
  plural,
  standardPanelMenu,
  useConsoleNavigation,
  usePanelDensity,
  usePanelPreference,
  useShowTechnicalIds,
  type FilterChipProps,
  type SegmentOption,
  type SummaryFact,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";

interface RosterPanelProps {
  agents: ConsoleAgent[];
  selectedMemberId?: string;
  onSelect: (agent: ConsoleAgent) => void;
  onChat: (agent: ConsoleAgent) => void;
  onDetails: (agent: ConsoleAgent) => void;
  onLifecycle: (identity: string, method: "meta/retire" | "meta/respawn" | "meta/reset") => void;
  canResetLifecycle?: boolean;
  actionLabels?: {
    chat?: string;
    inspect?: string;
    respawn?: string;
    retire?: string;
    reset?: string;
  };
  actionVisibility?: {
    chat?: boolean;
    inspect?: boolean;
    respawn?: boolean;
    retire?: boolean;
    reset?: boolean;
  };
  /** A fixed clock (tests, the gallery). */
  now?: () => number;
  /** The agent whose drawer is open when the panel first shows (the gallery). */
  defaultOpenMemberId?: string;
}

type Bucket = AgentBucket;
type Filter = "all" | Bucket;
/** A glance figure's filter: who is working, idle, or needs attention. */
type StateFilter = "working" | "idle" | "attention";
export type RosterView = "cards" | "list";
type RosterViewChoice = "auto" | RosterView;
const ROSTER_VIEW_CHOICES: readonly RosterViewChoice[] = ["auto", "cards", "list"];

/** From this pane width the roster shows cards unless someone chose. */
export const ROSTER_CARDS_MIN_WIDTH = 720;
/** Cards shown per group before "Show all". */
const CARDS_PER_GROUP = 48;
/** Rows rendered at once in a long list. */
const LIST_WINDOW_THRESHOLD = 200;

/** A health problem worth a chip: degraded or stalled (wedged), or broken. */
export function rosterHealthChip(agent: ConsoleAgent): { label: string; tone: Tone } | null {
  if (isRetired(agent)) return null;
  if (stateWord(agent) === "broken") return { label: "Broken", tone: "crit" };
  const health = agent.progress?.health;
  if (health === "wedged") return { label: "Stalled", tone: "crit" };
  if (health === "degraded") return { label: "Degraded", tone: "warn" };
  return null;
}

export function rosterDotTone(agent: ConsoleAgent): Tone {
  if (isRetired(agent)) return "neutral";
  const chip = rosterHealthChip(agent);
  if (chip) return chip.tone;
  const state = stateWord(agent);
  if (state === "active" || isWorking(agent)) return "ok";
  return "neutral";
}

/** What the agent is doing, in words: "writing a reply", "idle", "retired". */
export function rosterActivity(agent: ConsoleAgent): string {
  const state = stateWord(agent);
  if (state === "retiring") return "retiring";
  if (isRetired(agent)) return "retired";
  if (state === "broken") return "needs attention";
  // A wedged agent still has its run open; it is stuck, not working.
  if (agent.progress?.health === "wedged") return "stalled";
  const phase = responsePhaseWords(agent.response_phase);
  if (phase) return phase;
  const progress = agent.progress;
  if (progress?.run_state === "run_open") {
    return progress.in_flight_work > 1 ? `working on ${progress.in_flight_work} tasks` : "working";
  }
  if (progress?.run_state === "idle") return "idle";
  return state === "active" ? "ready" : "";
}

/**
 * The status pill of a roster card or row: a health problem first, then
 * Working (live), Retired, Not observed, Idle, else the member state.
 */
export function rosterStatus(health: AgentHealth, agent: ConsoleAgent, working: boolean): { word: string; tone: Tone } {
  if (health.problem || health.kind === "retired" || health.kind === "retiring") {
    return { word: health.word, tone: health.tone };
  }
  if (working) return { word: "Working", tone: "live" };
  if (health.kind === "unobserved") return { word: health.word, tone: "locked" };
  if (agent.progress?.run_state === "idle" || agent.response_phase === null) return { word: "Idle", tone: "neutral" };
  const state = stateWord(agent);
  return { word: state ? humanizeEnum(state) : "Ready", tone: state === "active" || !state ? "ok" : "neutral" };
}

function roleWords(agent: ConsoleAgent): string {
  return humanizeEnum(agent.role || agent.kind || "");
}

/** Role and group in words, the group left out where it only repeats the
 * role ("Coordinator · Coordinators") or the section the card sits in. */
export function roleAndGroup(agent: ConsoleAgent, section?: string): string {
  const role = roleWords(agent);
  const group = agent.group ? humanizeEnum(agent.group) : "";
  const stem = (value: string) => value.toLowerCase().replace(/s$/, "");
  const repeats = !group
    || (role && (stem(group).startsWith(stem(role)) || stem(role).startsWith(stem(group))))
    || (section && stem(group) === stem(section));
  return joinMeta(role, repeats ? "" : group);
}

function lastChange(agent: ConsoleAgent): { words: string; at: number | null } {
  const progress = agent.progress;
  const at = progress && progress.last_progress_at_ms > 0 ? progress.last_progress_at_ms : null;
  // A stalled agent is stuck, not working: name only when it last moved.
  if (progress?.health === "wedged") return { words: at ? "Last progress" : "", at };
  switch (progress?.last_progress_event) {
    case "execution_advanced":
      return { words: "Made progress", at };
    case "became_idle":
      return { words: "Finished and went idle", at };
    case "became_busy":
      return { words: "Started working", at };
    default:
      return { words: at ? "Last seen" : "", at };
  }
}

function peerKey(peer: unknown): string {
  if (typeof peer === "string") return peer.trim();
  if (peer && typeof peer === "object") {
    const record = peer as Record<string, unknown>;
    const value = record.identity ?? record.member_id ?? record.id ?? record.label ?? record.display_name ?? record.name;
    if (typeof value === "string") return value.trim();
  }
  return "";
}

function findAgent(agents: ConsoleAgent[], key: string): ConsoleAgent | undefined {
  const bare = key.split("/").pop() || key;
  return agents.find((agent) =>
    agent.identity === key || agent.member_id === key || agent.agent_id === key
    || agent.identity === bare || agent.member_id === bare);
}

/** A helper (spawned by another agent) or a side chat: drawn outlined. */
function isHelper(agent: ConsoleAgent): boolean {
  return Boolean(agent.side_chat_of || agent.labels?.side_chat_of || agent.labels?.spawned_by);
}

/** Everything one card, row and drawer show about an agent. */
interface RosterEntry {
  agent: ConsoleAgent;
  key: string;
  name: string;
  bucket: Bucket;
  activity: AgentActivity;
  health: AgentHealth;
  working: boolean;
  retired: boolean;
  helper: boolean;
  status: { word: string; tone: Tone };
  now: AgentNowLine;
  peers: string[];
}

function rosterEntry(agent: ConsoleAgent, ledger: ActivityLedgerReader, nowMs: number): RosterEntry {
  const activity = agentActivity(ledger, agent, nowMs);
  const health = agentHealthOf(agent, activity);
  const working = agentWorking(agent, activity);
  return {
    agent,
    key: agentKey(agent),
    name: agentDisplayTitle(agent),
    bucket: bucketOf(agent),
    activity,
    health,
    working,
    retired: isRetired(agent),
    helper: isHelper(agent),
    status: rosterStatus(health, agent, working),
    now: agentNowLine(agent, activity, nowMs),
    peers: (agent.wired_to || []).map(peerKey).filter(Boolean),
  };
}

function matchesState(entry: RosterEntry, filter: StateFilter | null): boolean {
  if (!filter) return true;
  if (entry.retired) return false;
  if (filter === "working") return entry.working;
  if (filter === "attention") return entry.health.problem;
  return !entry.working && !entry.health.problem;
}

const STATE_FILTER_WORDS: Record<StateFilter, { label: string; tone?: Tone }> = {
  working: { label: "Working", tone: "live" },
  idle: { label: "Idle" },
  attention: { label: "Needs attention", tone: "warn" },
};

export function RosterPanel({
  agents,
  selectedMemberId,
  onSelect,
  onChat,
  onDetails,
  onLifecycle,
  canResetLifecycle = false,
  actionLabels,
  actionVisibility,
  now,
  defaultOpenMemberId,
}: RosterPanelProps): React.JSX.Element {
  const ledger = useActivityLedger({ tickMs: 30_000 });
  const nowMs = now ? now() : ledger.nowMs;
  const navigation = useConsoleNavigation();
  const [density, setDensity] = usePanelDensity("roster");
  const [showIds, setShowIds] = useShowTechnicalIds("roster");
  const [viewChoice, setViewChoice] = usePanelPreference<RosterViewChoice>("meta-console-roster-view", "auto", ROSTER_VIEW_CHOICES);
  const measureRef = React.useRef<HTMLDivElement | null>(null);
  const width = useElementWidth(measureRef);
  const view: RosterView = viewChoice === "auto" ? (width >= ROSTER_CARDS_MIN_WIDTH ? "cards" : "list") : viewChoice;
  const [query, setQuery] = React.useState("");
  const [filter, setFilter] = React.useState<Filter>("all");
  const [stateFilter, setStateFilter] = React.useState<StateFilter | null>(null);
  const [sel, setSel] = React.useState<string>(defaultOpenMemberId || selectedMemberId || "");
  const [detailOpen, setDetailOpen] = React.useState(Boolean(defaultOpenMemberId));
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(() => new Set());

  React.useEffect(() => {
    if (selectedMemberId) setSel(selectedMemberId);
  }, [selectedMemberId]);

  // The ledger's version and the clock re-derive the entries.
  const entries = React.useMemo(
    () => agents.map((agent) => rosterEntry(agent, ledger, nowMs)),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [agents, ledger.version, nowMs],
  );

  const buckets = React.useMemo(() => {
    const counts = new Map<Bucket, number>();
    for (const entry of entries) counts.set(entry.bucket, (counts.get(entry.bucket) || 0) + 1);
    return BUCKET_ORDER.filter((bucket) => counts.has(bucket)).map((bucket) => ({ bucket, count: counts.get(bucket) || 0 }));
  }, [entries]);

  const rows = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    return entries.filter((entry) => {
      if (filter !== "all" && entry.bucket !== filter) return false;
      if (!matchesState(entry, stateFilter)) return false;
      if (!needle) return true;
      const agent = entry.agent;
      const hay = `${entry.name} ${agent.label} ${agent.role || ""} ${agent.kind || ""} ${agent.group || ""} ${agent.member_id} ${agent.identity || ""}`.toLowerCase();
      return hay.includes(needle);
    });
  }, [entries, query, filter, stateFilter]);

  const members = entries.filter((entry) => !entry.retired);
  const working = members.filter((entry) => entry.working);
  const problems = members.filter((entry) => entry.health.problem);
  const critical = problems.filter((entry) => entry.health.tone === "crit").length;
  const warning = problems.length - critical;
  const degradedCount = problems.filter((entry) => entry.health.kind === "degraded").length;
  const repairCount = problems.filter((entry) => entry.health.kind === "needs_repair").length;
  const brokenCount = problems.filter((entry) => entry.health.kind === "broken").length;
  const stalledCount = critical - brokenCount;
  const idle = members.filter((entry) => !entry.working && !entry.health.problem).length;
  const retiredCount = entries.length - members.length;
  const helpers = members.filter((entry) => entry.helper).length;
  const longLived = members.length - helpers;
  const workingLongLived = working.filter((entry) => !entry.helper).length;
  const teamEnd = hourCeiling(nowMs);
  const teamTurnsToday = React.useMemo(
    () => ledger.totals(null, { sinceMs: localDayStart(nowMs), untilMs: nowMs }).turnsStarted,
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ledger.version, Math.floor(nowMs / 60_000)],
  );
  const teamTurns = React.useMemo(
    () => ledger.series(null, "turnsStarted", { bins: 24, binMs: HOUR_MS, endMs: teamEnd }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ledger.version, teamEnd],
  );

  const active = entries.find((entry) => entry.agent.member_id === sel) ?? null;

  const select = (agent: ConsoleAgent) => {
    setSel(agent.member_id);
    setDetailOpen(true);
    onSelect(agent);
  };
  const openDetails = (agent: ConsoleAgent) => {
    if (navigation?.openAgent) navigation.openAgent(agentKey(agent), "details");
    else onDetails(agent);
  };
  const clearFilters = () => {
    setQuery("");
    setFilter("all");
    setStateFilter(null);
  };
  const toggleState = (next: StateFilter) => setStateFilter((current) => (current === next ? null : next));

  const filters: SegmentOption<Filter>[] = [
    { id: "all", label: "All", ariaLabel: "All", count: entries.length, testId: "roster-group:all" },
    ...buckets.map(({ bucket, count }) => ({ id: bucket, label: bucket, ariaLabel: bucket, count, testId: `roster-group:${bucket.toLowerCase()}` })),
  ];
  const stateChips: FilterChipProps[] = stateFilter
    ? [{
        label: STATE_FILTER_WORDS[stateFilter].label,
        tone: STATE_FILTER_WORDS[stateFilter].tone,
        pressed: true,
        onToggle: () => setStateFilter(null),
        testId: `roster-filter:${stateFilter}`,
      }]
    : [];

  const summary: Array<SummaryFact | null> = [
    { key: "agents", text: plural(members.length, "agent"), testId: "roster-summary:agents" },
    working.length ? { key: "working", text: `${working.length} working`, tone: "live", testId: "roster-summary:working" } : null,
    problems.length
      ? { key: "attention", text: `${problems.length} need${problems.length === 1 ? "s" : ""} attention`, tone: critical ? "crit" : "warn", testId: "roster-summary:attention" }
      : null,
    retiredCount ? { key: "retired", text: `${retiredCount} retired`, testId: "roster-summary:retired" } : null,
  ];

  const menuItems = standardPanelMenu({
    density,
    onDensityChange: setDensity,
    showTechnicalIds: showIds,
    onShowTechnicalIdsChange: setShowIds,
    getCopyData: () => entries.map((entry) => ({
      identity: entry.agent.identity,
      member_id: entry.agent.member_id,
      name: entry.name,
      status: entry.status.word,
      health: entry.health.kind,
      working: entry.working,
      today: entry.activity.today,
    })),
    testIdPrefix: "roster",
  });

  const empty = rows.length === 0 ? (
    entries.length === 0 ? (
      <EmptyState
        illustration="agents"
        title="No agents yet"
        body="Agents appear here once a team is running. Start a team, or check that this console can see it."
      />
    ) : (
      <EmptyState
        kind="filtered"
        title={query.trim() ? `No agents match "${query.trim()}"` : "No agents match these filters"}
        body={joinMeta(filter !== "all" ? filter : "", stateFilter ? STATE_FILTER_WORDS[stateFilter].label : "") || undefined}
        action={<Button size="sm" onClick={clearFilters}>Clear filters</Button>}
      />
    )
  ) : null;

  const cardFor = (entry: RosterEntry) => (
    <RosterCard
      key={entry.agent.member_id}
      entry={entry}
      agents={agents}
      selected={detailOpen && active?.agent.member_id === entry.agent.member_id}
      onSelect={() => select(entry.agent)}
      onChat={actionVisibility?.chat !== false ? () => onChat(entry.agent) : undefined}
      chatLabel={actionLabels?.chat || "Open chat"}
      onDetails={actionVisibility?.inspect !== false ? () => openDetails(entry.agent) : undefined}
      detailsLabel={actionLabels?.inspect || "Details"}
      showIds={showIds}
    />
  );

  let main: React.ReactNode;
  if (empty) {
    main = empty;
  } else if (view === "cards") {
    const groups: Array<{ id: string; label: string; entries: RosterEntry[] }> = [];
    for (const { bucket } of buckets) {
      const list = rows.filter((entry) => entry.bucket === bucket && !entry.retired);
      if (list.length) groups.push({ id: bucket, label: bucket, entries: list });
    }
    const retired = rows.filter((entry) => entry.retired);
    if (retired.length) groups.push({ id: "retired", label: "Retired", entries: retired });
    main = (
      <div className="roster__cards" data-testid="roster-cards">
        {groups.map((group) => {
          const open = expanded.has(group.id);
          const shown = open ? group.entries : group.entries.slice(0, CARDS_PER_GROUP);
          const headId = `roster-group-${group.id.toLowerCase()}`;
          return (
            <section key={group.id} className="roster__group" aria-labelledby={headId} data-group={group.id.toLowerCase()}>
              <div className="roster__group-head" id={headId}>
                {group.label} <Count value={group.entries.length} />
              </div>
              <ul className="roster__grid">
                {shown.map((entry) => (
                  <li key={entry.agent.member_id} className="roster__cell">
                    {cardFor(entry)}
                  </li>
                ))}
              </ul>
              {group.entries.length > shown.length ? (
                <button
                  type="button"
                  className="roster__more"
                  onClick={() => setExpanded((current) => new Set(current).add(group.id))}
                >
                  Show all {group.entries.length} {group.label.toLowerCase()}
                </button>
              ) : null}
            </section>
          );
        })}
      </div>
    );
  } else {
    main = (
      <RosterList
        rows={rows}
        selectedId={detailOpen ? active?.agent.member_id ?? null : null}
        onSelect={(entry) => select(entry.agent)}
        showIds={showIds}
      />
    );
  }

  const drawer = active ? (
    <RosterDrawer
      key={active.agent.member_id}
      entry={active}
      agents={agents}
      nowMs={nowMs}
      canResetLifecycle={canResetLifecycle}
      actionLabels={actionLabels}
      actionVisibility={actionVisibility}
      onChat={onChat}
      onDetails={openDetails}
      onLifecycle={onLifecycle}
      onSelectPeer={select}
      onClose={() => setDetailOpen(false)}
    />
  ) : null;

  return (
    <PanelShell className="roster" testId="roster-panel" density={density} showTechnicalIds={showIds}>
      <div ref={measureRef} className="roster__measure" aria-hidden="true" />
      <PanelHeader
        title="Roster"
        summary={summary}
        summaryTestId="roster-summary"
        view={{
          ariaLabel: "Roster view",
          value: view,
          onChange: (next: RosterView) => setViewChoice(next),
          testId: "roster-view",
          options: [
            { id: "cards", label: "Cards", glyph: "clusters", testId: "roster-view:cards" },
            { id: "list", label: "List", glyph: "list", testId: "roster-view:list" },
          ],
        }}
        menu={{ label: "More actions for Roster", items: menuItems, triggerTestId: "roster-menu" }}
      />
      {entries.length ? (
        <GlanceStrip ariaLabel="Roster at a glance" testId="roster-glance">
          <FigureTile
            hero
            label="Agents"
            value={members.length}
            meter={
              <BarcodeMeter
                filled={longLived}
                outlined={helpers}
                live={workingLongLived}
                width={width > 0 && width < 480 ? 72 : width > 0 && width < ROSTER_CARDS_MIN_WIDTH ? 104 : 132}
                label={`${plural(members.length, "agent")}: ${longLived} long-lived, ${plural(helpers, "helper")}, ${working.length} working now`}
              />
            }
            foot={<BarcodeLegend filled={longLived} outlined={helpers} live={workingLongLived} labels={{ live: "working" }} />}
            testId="roster-glance:agents"
          />
          <FigureTile
            label="Working"
            value={working.length}
            of={members.length}
            live={working.length > 0}
            foot={`${countWords(teamTurnsToday, "turn")} today`}
            title={`Show only the agents working now. ${seriesWords(teamTurns, teamEnd, "turn")}.`}
            onActivate={() => toggleState("working")}
            pressed={stateFilter === "working"}
            testId="roster-glance:working"
          />
          <FigureTile
            label="Idle"
            value={idle}
            onActivate={() => toggleState("idle")}
            pressed={stateFilter === "idle"}
            title="Show only idle agents"
            testId="roster-glance:idle"
          />
          <FigureTile
            label="Need attention"
            value={problems.length}
            tone={critical ? "crit" : warning ? "warn" : undefined}
            foot={
              problems.length ? (
                <span className="roster__attention-foot">
                  {degradedCount ? <span><StatusMark tone="warn" /> {degradedCount} degraded</span> : null}
                  {repairCount ? <span><StatusMark tone="warn" /> {repairCount} need{repairCount === 1 ? "s" : ""} repair</span> : null}
                  {stalledCount ? <span><StatusMark tone="crit" /> {stalledCount} stalled</span> : null}
                  {brokenCount ? <span><StatusMark tone="crit" /> {brokenCount} broken</span> : null}
                </span>
              ) : "All healthy"
            }
            onActivate={() => toggleState("attention")}
            pressed={stateFilter === "attention"}
            title="Show only the agents that need attention"
            testId="roster-glance:attention"
          />
        </GlanceStrip>
      ) : null}
      {entries.length ? (
        <FilterBar
          ariaLabel="Roster filters"
          className="roster__filters"
          search={{ label: "Search agents", value: query, onChange: setQuery, placeholder: "Search agents", testId: "roster-search" }}
          chips={stateChips}
          modified={Boolean(query.trim() || filter !== "all" || stateFilter)}
          onClear={clearFilters}
          testId="roster-filters"
        >
          {filters.length > 2 ? (
            <SegmentedControl ariaLabel="Agent groups" value={filter} onChange={setFilter} options={filters} className="roster__groups" />
          ) : null}
        </FilterBar>
      ) : null}
      <DrawerLayout
        className="roster__layout"
        main={<div className="roster__main" data-view={view}>{main}</div>}
        drawer={drawer}
        open={detailOpen && Boolean(active)}
        onClose={() => setDetailOpen(false)}
        persistKey="roster"
        defaultWidth={360}
        backLabel="All agents"
        backTestId="roster-detail-back"
        testId="roster-layout"
      />
    </PanelShell>
  );
}

/** Wired peers by name, the first few, then "+N". */
function PeerNames({ peers, agents, max = 3 }: { peers: string[]; agents: ConsoleAgent[]; max?: number }) {
  if (!peers.length) return null;
  const shown = peers.slice(0, max);
  return (
    <span className="roster__relations" aria-label={`Connected to ${plural(peers.length, "agent")}`}>
      <KitConnections />
      {shown.map((peer) => {
        const target = findAgent(agents, peer);
        return <AgentName key={peer} id={target?.identity || target?.member_id || peer} link className="roster__relation" />;
      })}
      {peers.length > shown.length ? <span className="roster__relation-more">+{peers.length - shown.length}</span> : null}
    </span>
  );
}

function KitConnections() {
  return <KitGlyph name="link" className="roster__relations-mark" />;
}

function RosterCard({
  entry,
  agents,
  selected,
  onSelect,
  onChat,
  chatLabel,
  onDetails,
  detailsLabel,
  showIds,
}: {
  entry: RosterEntry;
  agents: ConsoleAgent[];
  selected: boolean;
  onSelect: () => void;
  onChat?: () => void;
  chatLabel: string;
  onDetails?: () => void;
  detailsLabel: string;
  showIds: boolean;
}) {
  const { agent, activity } = entry;
  const identity = agent.identity || agent.member_id;
  const turns = activity.today.turnsStarted;
  const sideChatOf = agent.side_chat_of || agent.labels?.side_chat_of;
  return (
    <EntityCard
      kind="agent"
      testId={`roster-card:${agent.member_id}`}
      className={entry.retired ? "roster__card is-retired" : "roster__card"}
      dataAttributes={{ "data-state": stateWord(agent) || "unknown", "data-health": agent.progress?.health || undefined, "data-status": entry.health.kind }}
      mark={<AgentAvatar name={entry.name} size={28} live={entry.working} />}
      title={<span title={identity}>{entry.name}</span>}
      activateLabel={entry.name}
      subtitle={joinMeta(roleAndGroup(agent, entry.bucket), sideChatOf ? "side chat" : "") || undefined}
      status={<StatusPill tone={entry.status.tone}>{entry.status.word}</StatusPill>}
      now={{ text: entry.now.text, live: entry.now.live }}
      trend={{
        values: activity.hourly,
        label: seriesWords(activity.hourly, activity.endMs, "event"),
        live: entry.working,
        kind: "bars",
      }}
      figures={[countWords(turns, "turn") + " today"]}
      time={activity.lastActivityMs ? <RelativeTime value={activity.lastActivityMs} /> : undefined}
      relations={entry.peers.length ? <PeerNames peers={entry.peers} agents={agents} /> : undefined}
      technical={showIds ? [{ label: "Identity", value: identity }] : undefined}
      actions={
        onChat || onDetails ? (
          <>
            {onChat ? <IconButton glyph="chat" size="sm" label={`${chatLabel}: ${entry.name}`} onClick={onChat} data-testid={`roster-card-chat:${agent.member_id}`} /> : null}
            {onDetails ? <IconButton glyph="open" size="sm" label={`${detailsLabel}: ${entry.name}`} onClick={onDetails} data-testid={`roster-card-details:${agent.member_id}`} /> : null}
          </>
        ) : undefined
      }
      selected={selected}
      live={entry.working}
      onActivate={onSelect}
    />
  );
}

/** The list view: one row per agent; a long list renders only the rows in view. */
function RosterList({
  rows,
  selectedId,
  onSelect,
  showIds,
}: {
  rows: RosterEntry[];
  selectedId: string | null;
  onSelect: (entry: RosterEntry) => void;
  showIds: boolean;
}) {
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const windowed = rows.length > LIST_WINDOW_THRESHOLD;
  const rowHeight = 48;
  const [range, setRange] = React.useState({ start: 0, end: Math.min(rows.length, 60) });
  React.useLayoutEffect(() => {
    if (!windowed) return undefined;
    const element = scrollRef.current;
    if (!element) return undefined;
    const update = () => {
      const top = element.scrollTop;
      const height = element.clientHeight || 800;
      const start = Math.max(0, Math.floor(top / rowHeight) - 10);
      const end = Math.min(rows.length, Math.ceil((top + height) / rowHeight) + 10);
      setRange((current) => (current.start === start && current.end === end ? current : { start, end }));
    };
    update();
    element.addEventListener("scroll", update, { passive: true });
    return () => element.removeEventListener("scroll", update);
  }, [windowed, rows.length]);
  const start = windowed ? range.start : 0;
  const end = windowed ? Math.min(range.end, rows.length) : rows.length;
  return (
    <div ref={scrollRef} className="roster__list" data-testid="roster-list" data-windowed={windowed ? "true" : undefined}>
      {start > 0 ? <div style={{ height: start * rowHeight }} aria-hidden="true" /> : null}
      {rows.slice(start, end).map((entry) => {
        const { agent, activity } = entry;
        const identity = agent.identity || agent.member_id;
        return (
          <EntityRow
            key={agent.member_id}
            kind="agent"
            testId={`roster-row:${agent.member_id}`}
            className={entry.retired ? "roster__row is-retired" : "roster__row"}
            dataAttributes={{ "data-state": stateWord(agent) || "unknown", "data-health": agent.progress?.health || undefined, "data-status": entry.health.kind }}
            mark={<AgentAvatar name={entry.name} size={22} live={entry.working} />}
            title={<span title={identity}>{entry.name}</span>}
            activateLabel={entry.name}
            subtitle={joinMeta(roleWords(agent), rosterActivity(agent)) || undefined}
            trend={{
              values: activity.hourly,
              label: seriesWords(activity.hourly, activity.endMs, "event"),
              live: entry.working,
              kind: "bars",
            }}
            figures={showIds ? [<span className="roster__id" key="id">{identity}</span>] : undefined}
            status={<StatusPill tone={entry.status.tone} variant="quiet">{entry.status.word}</StatusPill>}
            time={activity.lastActivityMs ? <RelativeTime value={activity.lastActivityMs} /> : undefined}
            selected={selectedId === agent.member_id}
            live={entry.working}
            onActivate={() => onSelect(entry)}
          />
        );
      })}
      {end < rows.length ? <div style={{ height: (rows.length - end) * rowHeight }} aria-hidden="true" /> : null}
    </div>
  );
}

const HEAT_ROWS: Array<{ key: string; label: string; metric: "turnsStarted" | "toolCalls" | "messagesSent" | "failures" }> = [
  { key: "turns", label: "Turns", metric: "turnsStarted" },
  { key: "tools", label: "Tools", metric: "toolCalls" },
  { key: "messages", label: "Messages", metric: "messagesSent" },
  { key: "failures", label: "Failures", metric: "failures" },
];

function RosterDrawer({
  entry,
  agents,
  nowMs,
  canResetLifecycle,
  actionLabels,
  actionVisibility,
  onChat,
  onDetails,
  onLifecycle,
  onSelectPeer,
  onClose,
}: {
  entry: RosterEntry;
  agents: ConsoleAgent[];
  nowMs: number;
  canResetLifecycle: boolean;
  actionLabels: RosterPanelProps["actionLabels"];
  actionVisibility: RosterPanelProps["actionVisibility"];
  onChat: (agent: ConsoleAgent) => void;
  onDetails: (agent: ConsoleAgent) => void;
  onLifecycle: RosterPanelProps["onLifecycle"];
  onSelectPeer: (agent: ConsoleAgent) => void;
  onClose: () => void;
}) {
  const ledger = useActivityLedger();
  const { agent, activity } = entry;
  const identity = agent.identity || agent.member_id;
  const [confirming, setConfirming] = React.useState<ConfirmedLifecycleAction | null>(null);
  const change = lastChange(agent);
  const activityWords = rosterActivity(agent);
  const inFlight = agent.progress?.in_flight_work ?? 0;
  const showRespawn = actionVisibility?.respawn !== false && agent.affordances?.can_respawn && restartOffered(agent);
  const showReset = actionVisibility?.reset !== false && canResetLifecycle && restartOffered(agent);
  const showRetire = actionVisibility?.retire !== false && agent.affordances?.can_retire;
  const sideChatOf = agent.side_chat_of || agent.labels?.side_chat_of;
  const spawnedBy = agent.labels?.spawned_by;
  const heatRows = React.useMemo(
    () => HEAT_ROWS.map((row) => ({
      key: row.key,
      label: row.label,
      values: ledger.series(entry.key, row.metric, { bins: 24, binMs: HOUR_MS, endMs: activity.endMs }),
    })),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [ledger.version, entry.key, activity.endMs],
  );
  const heatColumns = React.useMemo(
    () => Array.from({ length: 24 }, (_, index) => hourLabel(activity.endMs - (24 - index) * HOUR_MS)),
    [activity.endMs],
  );
  const busiest = heatRows[0].values.reduce((best, value, index, values) => (value > values[best] ? index : best), 0);
  const confirm = (action: ConfirmedLifecycleAction) => {
    setConfirming(null);
    onLifecycle(identity, action === "retire" ? "meta/retire" : "meta/reset");
  };
  return (
    <DetailDrawer
      testId={`roster-detail:${agent.member_id}`}
      className="roster__drawer"
      title={<span title={identity}>{entry.name}</span>}
      ariaLabel={`${entry.name}, details`}
      mark={<AgentAvatar name={entry.name} size={36} live={entry.working} />}
      status={<StatusPill tone={entry.status.tone}>{entry.status.word}</StatusPill>}
      subtitle={joinMeta(humanizeEnum(agent.role || agent.kind || ""), agent.group ? humanizeEnum(agent.group) : "") || undefined}
      onClose={onClose}
      closeTestId="roster-detail-close"
      onOpenInPane={actionVisibility?.inspect !== false ? () => onDetails(agent) : undefined}
      openInPaneLabel={`Open ${entry.name}'s details beside`}
      technical={[
        { label: "Identity", value: agent.identity },
        agent.member_id !== agent.identity ? { label: "Member", value: agent.member_id } : null,
        { label: "Session", value: agent.session_id },
        typeof agent.generation === "number" ? { label: "Generation", value: agent.generation } : null,
        { label: "Kind", value: agent.kind, plain: true },
        { label: "Profile", value: agent.role, plain: true },
      ]}
    >
      <div className="roster__actions">
        <div className="cc-btn-row">
          {actionVisibility?.chat !== false ? (
            <Button variant="primary" onClick={() => onChat(agent)}>{actionLabels?.chat || "Open chat"}</Button>
          ) : null}
          {actionVisibility?.inspect !== false ? (
            <Button onClick={() => onDetails(agent)}>{actionLabels?.inspect || "Details"}</Button>
          ) : null}
        </div>
        {showRespawn || showReset || showRetire ? (
          <div className="cc-btn-row" role="group" aria-label="Lifecycle">
            {showRespawn ? (
              <Button size="sm" onClick={() => onLifecycle(identity, "meta/respawn")}>{actionLabels?.respawn || "Respawn"}</Button>
            ) : null}
            {showReset ? (
              <Button
                size="sm"
                aria-expanded={confirming === "reset"}
                data-testid={`roster-action:${agent.member_id}:reset`}
                onClick={() => setConfirming("reset")}
              >
                {actionLabels?.reset || "Reset"}
              </Button>
            ) : null}
            {showRetire ? (
              <Button
                size="sm"
                variant="danger"
                aria-expanded={confirming === "retire"}
                data-testid={`roster-action:${agent.member_id}:retire`}
                onClick={() => setConfirming("retire")}
              >
                {actionLabels?.retire || "Retire"}
              </Button>
            ) : null}
          </div>
        ) : null}
        {confirming ? (
          <LifecycleConfirm
            action={confirming}
            name={entry.name}
            actionLabel={confirming === "retire" ? actionLabels?.retire : actionLabels?.reset}
            onConfirm={() => confirm(confirming)}
            onCancel={() => setConfirming(null)}
            testId={`roster-${confirming}-confirm:${agent.member_id}`}
            confirmTestId={`roster-${confirming}-confirm-yes:${agent.member_id}`}
            cancelTestId={`roster-${confirming}-confirm-cancel:${agent.member_id}`}
          />
        ) : null}
      </div>
      <DrawerSection label="Now" testId="roster-detail-now">
        <p className="roster__now" data-live={entry.now.live ? "true" : undefined}>
          {entry.now.live ? <StatusMark tone="live" /> : null}
          {entry.now.text}
        </p>
        <KeyValueList
          items={[
            { label: "Status", value: activityWords ? activityWords.charAt(0).toUpperCase() + activityWords.slice(1) : humanizeEnum(agent.state || "unknown") },
            agent.progress ? { label: "In flight", value: inFlight === 0 ? "Nothing" : plural(inFlight, "task") } : null,
            change.words
              ? {
                  label: "Last change",
                  value: (
                    <>
                      {change.words}
                      {change.at ? <>{", "}<RelativeTime value={change.at} /></> : null}
                    </>
                  ),
                }
              : null,
            agent.addressable === false ? { label: "Reachability", value: "Internal only" } : null,
            sideChatOf ? { label: "Side chat of", value: <AgentName id={sideChatOf} link /> } : null,
            spawnedBy ? { label: "Started by", value: <AgentName id={spawnedBy} link /> } : null,
          ]}
        />
      </DrawerSection>
      <DrawerSection
        label="Activity · 24h"
        action={`${countWords(activity.today.turnsStarted, "turn")} today`}
        testId="roster-detail-activity"
      >
        <ActivityBars
          values={activity.hourly}
          end={activity.endMs}
          width={300}
          height={48}
          live={entry.working}
          unit="events"
          label={seriesWords(activity.hourly, activity.endMs, "event")}
        />
      </DrawerSection>
      <DrawerSection label="By hour" action={heatRows[0].values[busiest] ? `busiest ${hourLabel(activity.endMs - (24 - busiest) * HOUR_MS)}` : undefined} testId="roster-detail-heat">
        <HeatStrip
          rows={heatRows}
          columns={heatColumns}
          cell={9}
          gap={2}
          rowLabelWidth={56}
          columnTick={(index) => (index % 6 === 0 ? heatColumns[index].slice(0, 2) : null)}
          label={`${entry.name}'s activity per hour over the last 24 hours: ${heatRows.map((row) => `${row.values.reduce((a, b) => a + b, 0)} ${row.label.toLowerCase()}`).join(", ")}`}
          formatCell={(row, column, value) => `${row.label} · ${heatColumns[column]} · ${value}`}
          testId="roster-detail-heat-strip"
        />
      </DrawerSection>
      {entry.peers.length ? (
        <DrawerSection label="Connected to" count={entry.peers.length} testId="roster-detail-peers">
          <div className="cc-chips roster__peers">
            {entry.peers.map((peer) => {
              const target = findAgent(agents, peer);
              return target ? (
                <button
                  key={peer}
                  type="button"
                  className="roster__peer"
                  onClick={() => onSelectPeer(target)}
                  title={peer}
                >
                  <AgentName id={target.identity || target.member_id} avatar={18} />
                </button>
              ) : (
                <span key={peer} className="roster__peer is-static">
                  <AgentName id={peer} />
                </span>
              );
            })}
          </div>
        </DrawerSection>
      ) : null}
    </DetailDrawer>
  );
}
