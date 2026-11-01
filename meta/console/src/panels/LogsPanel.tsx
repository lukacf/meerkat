import React from "react";
import type { ConsoleFrame } from "../types";
import {
  AgentAvatar,
  AgentName,
  Button,
  EmptyState,
  FilterBar,
  IconButton,
  KitGlyph,
  NewItemsPill,
  PanelHeader,
  PanelShell,
  SelectField,
  StatusMark,
  StatusPill,
  TechnicalDetails,
  TechnicalId,
  agentDisplayName,
  formatAbsoluteTime,
  formatTimeRange,
  plural,
  standardPanelMenu,
  useConsoleNavigation,
  usePanelDensity,
  usePanelPreference,
  useShowTechnicalIds,
  EMPTY_AGENT_DIRECTORY,
  type AgentDirectory,
  type MenuEntry,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import { SeverityStrip, severityBins, type TimeRange } from "../../../packages/console-components/src/viz/charts/severity-strip";
import {
  buildActivityEntries,
  entryMatches,
  entrySentence,
  mergeFrameSources,
  type ActivityEntry,
} from "./activity/activity-model";
import {
  LEVEL_WORDS,
  formatFrameData,
  formatLogDuration,
  formatLogTime,
  groupByInteraction,
  hasStructuredOutput,
  isLogFrameVisible,
  levelCounts,
  logLevel,
  summarizeLogFrame,
  type LogGroup,
  type LogLevel,
} from "./activity/logs-model";
import { useVirtualRows } from "./activity/use-virtual-rows";

export { isLogFrameVisible, logIdentityLabel, sanitizeLogFrameData, summarizeLogFrame } from "./activity/logs-model";

interface LogsPanelProps {
  /** The console's live activity buffer, newest first. */
  frames: ConsoleFrame[];
  /** The agents' loaded history logs, in any order. */
  historyFrames?: readonly ConsoleFrame[];
  /** Resolves identity keys to display names; keys show without their kind
   * prefix when absent. */
  agentDirectory?: AgentDirectory;
  /** Opens an agent's chat or details while the console provides no
   * navigation context. */
  onOpenAgent?: (identity: string, how: "details" | "chat") => void;
}

type LevelFilter = "all" | LogLevel;

const LEVEL_TONE: Readonly<Record<LogLevel, Tone>> = { info: "neutral", warn: "warn", error: "crit" };
const STRIP_BINS = 60;
const ROW_HEIGHT = { comfortable: 30, compact: 24 } as const;

type ListItem =
  | { kind: "row"; key: string; entry: ActivityEntry; group?: LogGroup }
  | { kind: "group"; key: string; group: LogGroup };

function levelTone(level: LogLevel): Tone {
  return LEVEL_TONE[level];
}

/** The detail's facts line: who, which event, which interaction, when. */
function DetailFacts({ entry, directory }: { entry: ActivityEntry; directory: AgentDirectory }) {
  const frame = entry.frame;
  const identity = frame.identity?.trim();
  return (
    <div className="logline__facts">
      <span className="logline__fact">
        <span className="logline__fact-label">Agent</span>
        {identity && identity !== "_system" ? (
          <AgentName id={identity} avatar={18} link directory={directory} />
        ) : (
          <span>System</span>
        )}
      </span>
      <span className="logline__fact">
        <span className="logline__fact-label">Event</span>
        <code className="logline__code">{frame.event}</code>
      </span>
      {entry.interactionId ? (
        <span className="logline__fact">
          <span className="logline__fact-label">Interaction</span>
          <TechnicalId value={entry.interactionId} label="interaction ID" />
        </span>
      ) : null}
      {entry.atMs ? (
        <span className="logline__fact">
          <span className="logline__fact-label">Time</span>
          <time dateTime={new Date(entry.atMs).toISOString()}>{formatAbsoluteTime(entry.atMs)}</time>
        </span>
      ) : null}
    </div>
  );
}

export function LogsPanel({
  frames,
  historyFrames,
  agentDirectory = EMPTY_AGENT_DIRECTORY,
  onOpenAgent,
}: LogsPanelProps): React.JSX.Element {
  const navigation = useConsoleNavigation();
  const [density, setDensity] = usePanelDensity("logs");
  const [showTechnicalIds, setShowTechnicalIds] = useShowTechnicalIds("logs");
  const [grouped, setGrouped] = usePanelPreference<"on" | "off">("meta-console-logs-group", "off", ["on", "off"]);
  const [internal, setInternal] = usePanelPreference<"on" | "off">("meta-console-logs-internal", "off", ["on", "off"]);
  const [query, setQuery] = React.useState("");
  const [level, setLevel] = React.useState<LevelFilter>("all");
  const [agent, setAgent] = React.useState("");
  const [range, setRange] = React.useState<TimeRange | null>(null);
  const [expanded, setExpanded] = React.useState<Set<string>>(() => new Set());
  const [openGroups, setOpenGroups] = React.useState<Set<string>>(() => new Set());
  const [target, setTarget] = React.useState<string | null>(null);
  // Paused keeps the frames shown when the operator paused, so rows stop
  // moving while they read; following live shows every new frame.
  const [paused, setPaused] = React.useState<{ frames: ConsoleFrame[]; history?: readonly ConsoleFrame[] } | null>(null);
  const streamRef = React.useRef<HTMLDivElement | null>(null);
  const nameOf = React.useCallback((key: string) => agentDisplayName(key, agentDirectory) || key, [agentDirectory]);

  const liveFrames = paused?.frames ?? frames;
  const liveHistory = paused ? paused.history : historyFrames;
  const merged = React.useMemo(
    () => mergeFrameSources(liveFrames, liveHistory).filter(isLogFrameVisible),
    [liveFrames, liveHistory],
  );
  const entries = React.useMemo(
    () => buildActivityEntries(merged, { directory: agentDirectory, includeInternal: internal === "on" }),
    [merged, agentDirectory, internal],
  );
  // While paused, the events that arrived since, for the "N new" pill.
  const newWhilePaused = React.useMemo(() => {
    if (!paused) return 0;
    const newest = entries[0]?.atMs ?? 0;
    const current = mergeFrameSources(frames, historyFrames).filter((frame) => isLogFrameVisible(frame) && (frame.timestampMs ?? 0) > newest);
    return buildActivityEntries(current, { directory: agentDirectory, includeInternal: internal === "on" }).length;
  }, [paused, entries, frames, historyFrames, agentDirectory, internal]);

  const loaded = React.useMemo(() => {
    if (!entries.length) return null;
    const oldest = entries[entries.length - 1].atMs;
    const newest = Math.max(entries[0].atMs, paused ? entries[0].atMs : Date.now());
    return { start: oldest, end: Math.max(newest, oldest + 60_000) };
  }, [entries, paused]);

  const agents = React.useMemo(() => {
    const seen = new Map<string, number>();
    for (const entry of entries) {
      const key = entry.frame.identity?.trim();
      if (key && key !== "_system") seen.set(key, (seen.get(key) ?? 0) + 1);
    }
    return [...seen.entries()].sort(([a], [b]) => nameOf(a).localeCompare(nameOf(b)));
  }, [entries, nameOf]);

  // Filters before the level: the level chips count within them, and the
  // strip draws them.
  const scoped = React.useMemo(() => entries.filter((entry) => {
    if (agent && entry.frame.identity !== agent && entry.actor !== agent) return false;
    if (query && !entryMatches(entry, query, nameOf) && !summarizeLogFrame(entry.frame).toLowerCase().includes(query.trim().toLowerCase())) return false;
    return true;
  }), [entries, agent, query, nameOf]);
  const inRange = React.useMemo(
    () => (range ? scoped.filter((entry) => entry.atMs >= range.start && entry.atMs <= range.end) : scoped),
    [scoped, range],
  );
  const counts = React.useMemo(() => levelCounts(inRange), [inRange]);
  const rows = React.useMemo(
    () => (level === "all" ? inRange : inRange.filter((entry) => logLevel(entry) === level)),
    [inRange, level],
  );
  const totals = React.useMemo(() => levelCounts(entries), [entries]);
  const bins = React.useMemo(() => (loaded
    ? severityBins(scoped.filter((entry) => level === "all" || logLevel(entry) === level)
      .map((entry) => ({ t: entry.atMs, severity: entry.severity })), loaded.start, loaded.end, STRIP_BINS)
    : []), [scoped, loaded, level]);

  const items = React.useMemo<ListItem[]>(() => {
    if (grouped !== "on") return rows.map((entry) => ({ kind: "row" as const, key: entry.rowKey, entry }));
    const list: ListItem[] = [];
    for (const group of groupByInteraction(rows)) {
      if (group.entries.length === 1) {
        list.push({ kind: "row", key: group.entries[0].rowKey, entry: group.entries[0] });
        continue;
      }
      list.push({ kind: "group", key: `group:${group.key}`, group });
      if (openGroups.has(group.key)) {
        for (const entry of group.entries) list.push({ kind: "row", key: entry.rowKey, entry, group });
      }
    }
    return list;
  }, [rows, grouped, openGroups]);

  const rowHeight = ROW_HEIGHT[density];
  const virtual = useVirtualRows({
    count: items.length,
    keyAt: (index) => items[index].key,
    estimate: (index) => {
      const item = items[index];
      return item.kind === "row" && expanded.has(item.key) ? rowHeight + 320 : rowHeight;
    },
    scrollRef: streamRef,
  });

  React.useEffect(() => {
    if (!target) return undefined;
    const timer = setTimeout(() => setTarget(null), 1600);
    return () => clearTimeout(timer);
  }, [target]);

  const toggle = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  const toggleGroup = (key: string) =>
    setOpenGroups((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  const openChat = (identity: string) => {
    if (navigation?.openAgent) navigation.openAgent(identity, "chat");
    else onOpenAgent?.(identity, "chat");
  };
  const canOpenChat = Boolean(navigation?.openAgent || onOpenAgent);
  const showBin = (index: number) => {
    const bin = bins[index];
    if (!bin) return;
    const position = items.findIndex((item) => {
      const entry = item.kind === "row" ? item.entry : item.group.entries[0];
      return entry.atMs >= bin.start && entry.atMs < bin.end + 1;
    });
    if (position < 0) return;
    setTarget(items[position].key);
    virtual.scrollToIndex(position, "center");
  };
  const clearFilters = () => {
    setQuery("");
    setLevel("all");
    setAgent("");
    setRange(null);
  };
  const filtered = Boolean(query) || level !== "all" || Boolean(agent) || Boolean(range);
  const follow = paused === null;

  const menuItems: MenuEntry[] = standardPanelMenu({
    viewOptions: [
      {
        kind: "checkbox",
        id: "group",
        label: "Group by interaction",
        checked: grouped === "on",
        onChange: (on) => setGrouped(on ? "on" : "off"),
        testId: "logs-menu:group",
      },
      {
        kind: "checkbox",
        id: "internal",
        label: "Show internal events",
        checked: internal === "on",
        onChange: (on) => setInternal(on ? "on" : "off"),
        testId: "logs-menu:internal",
      },
    ],
    density,
    onDensityChange: setDensity,
    showTechnicalIds,
    onShowTechnicalIdsChange: setShowTechnicalIds,
    getCopyData: () => rows.slice(0, 500).map((entry) => ({
      at: new Date(entry.atMs).toISOString(),
      level: logLevel(entry),
      agent: entry.frame.identity ?? null,
      event: entry.frame.event,
      sentence: entrySentence(entry, nameOf),
      snippet: entry.snippet ?? null,
      interaction_id: entry.interactionId ?? null,
      frame_id: entry.frame.id,
    })),
    testIdPrefix: "logs",
  });

  const renderRow = (item: Extract<ListItem, { kind: "row" }>) => {
    const { entry } = item;
    const frame = entry.frame;
    const rowLevel = logLevel(entry);
    const isOpen = expanded.has(item.key);
    const identity = frame.identity?.trim() || "";
    const lineId = frame.id || item.key;
    const sentence = entrySentence(entry, nameOf);
    const duration = formatLogDuration(entry.durationMs);
    return (
      <div
        key={item.key}
        ref={virtual.measureRef(item.key)}
        className={[
          "logline",
          `logline--${rowLevel}`,
          isOpen ? "is-open" : "",
          item.group ? "logline--in-group" : "",
          target === item.key ? "is-target" : "",
        ].filter(Boolean).join(" ")}
        data-testid={`log-line:${lineId}`}
      >
        <button
          type="button"
          className="logline__row"
          onClick={() => toggle(item.key)}
          aria-expanded={isOpen}
          data-testid={`log-line:${lineId}:toggle`}
        >
          <span className="logline__chevron" aria-hidden="true" />
          <span className="logline__t" title={formatAbsoluteTime(entry.atMs) || undefined}>{formatLogTime(entry.atMs)}</span>
          <span className={`logline__lvl logline__lvl--${rowLevel}`}>
            <StatusMark tone={levelTone(rowLevel)} />
            {LEVEL_WORDS[rowLevel]}
          </span>
          <span className="cc-agent logline__src" title={identity || undefined}>
            {identity && identity !== "_system" ? <AgentAvatar name={nameOf(identity)} size={18} /> : null}
            <span className="cc-agent__name">{identity && identity !== "_system" ? nameOf(identity) : "System"}</span>
          </span>
          <span className="logline__fam" title={frame.event}>{entry.familyWord}</span>
          <span className="logline__msg" title={entry.snippet ? `${sentence}: ${entry.snippet}` : sentence}>
            <span className="logline__sentence">{sentence}</span>
            {entry.snippet ? <span className="logline__snippet">{`: ${entry.snippet}`}</span> : null}
            {hasStructuredOutput(frame) ? <span className="logline__badge" title="Carries structured output">structured</span> : null}
          </span>
          <span className="logline__dur">{duration}</span>
        </button>
        {isOpen ? (
          <div className="logline__detail" data-testid={`log-line:${lineId}:detail`}>
            <DetailFacts entry={entry} directory={agentDirectory} />
            <p className="logline__words">
              {sentence}
              {entry.snippet ? <span className="logline__snippet">{`: ${entry.snippet}`}</span> : null}
            </p>
            <code className="logline__summary">{summarizeLogFrame(frame)}</code>
            <pre className="logline__data">{formatFrameData(frame)}</pre>
            <div className="logline__actions">
              {canOpenChat && identity && identity !== "_system" ? (
                <Button size="sm" onClick={() => openChat(identity)} data-testid={`log-line:${lineId}:chat`}>
                  <KitGlyph name="chat" />
                  Open in chat
                </Button>
              ) : null}
              <TechnicalDetails
                className="logline__tech"
                items={[
                  { label: "Frame", value: frame.id },
                  { label: "Version", value: frame.frameVersion ?? null, plain: true },
                  { label: "Cursor", value: frame.cursor },
                  { label: "Run", value: entry.runId },
                  { label: "Session", value: frame.sessionId },
                  { label: "Source", value: frame.sourceKind, plain: true },
                  entry.frames.length > 1 ? { label: "Copies", value: `${entry.frames.length}: ${entry.frames.map((copy) => copy.sourceKind || "live").join(", ")}`, plain: true } : null,
                ]}
              />
            </div>
          </div>
        ) : null}
      </div>
    );
  };

  const renderGroup = (item: Extract<ListItem, { kind: "group" }>) => {
    const { group } = item;
    const open = openGroups.has(group.key);
    const lead = group.entries[group.entries.length - 1];
    const actor = group.actor;
    const span = group.endMs - group.startMs;
    return (
      <div
        key={item.key}
        ref={virtual.measureRef(item.key)}
        className={["logs__group", `logs__group--${group.level}`, open ? "is-open" : "", target === item.key ? "is-target" : ""].filter(Boolean).join(" ")}
        data-testid={`logs-group:${group.interactionId ?? group.key}`}
      >
        <button type="button" className="logs__group-row" aria-expanded={open} onClick={() => toggleGroup(group.key)}>
          <span className="logline__chevron" aria-hidden="true" />
          <span className="logline__t">{formatLogTime(group.endMs)}</span>
          <span className={`logline__lvl logline__lvl--${group.level}`}>
            <StatusMark tone={levelTone(group.level)} />
            {LEVEL_WORDS[group.level]}
          </span>
          <span className="cc-agent logline__src">
            {actor ? <AgentAvatar name={nameOf(actor)} size={18} /> : null}
            <span className="cc-agent__name">{actor ? nameOf(actor) : "System"}</span>
          </span>
          <span className="logs__group-words">
            {entrySentence(lead, nameOf)}
          </span>
          <span className="logs__group-meta">
            {plural(group.entries.length, "event")}
            {span >= 1000 ? ` · ${formatLogDuration(span)}` : ""}
          </span>
        </button>
      </div>
    );
  };

  const summaryFacts = [
    { key: "events", text: filtered ? `${rows.length.toLocaleString()} of ${plural(entries.length, "event")}` : plural(entries.length, "event") },
    totals.warn ? { key: "warn", text: plural(totals.warn, "warning"), tone: "warn" as const } : null,
    totals.error ? { key: "error", text: plural(totals.error, "error"), tone: "crit" as const } : null,
    {
      key: "follow",
      text: follow ? "Live" : newWhilePaused ? `Paused, ${plural(newWhilePaused, "new event")}` : "Paused",
      tone: follow ? ("live" as const) : undefined,
      announce: false,
    },
  ];

  return (
    <PanelShell className="logs" testId="logs-panel" density={density} showTechnicalIds={showTechnicalIds}>
      <PanelHeader
        title="Logs"
        summary={summaryFacts}
        summaryTestId="logs-summary"
        actions={(
          <Button
            size="sm"
            className="logs__follow"
            aria-pressed={follow}
            onClick={() => setPaused((current) => (current ? null : { frames, history: historyFrames }))}
            data-testid="logs-follow"
          >
            <StatusMark tone={follow ? "live" : "neutral"} />
            Follow live
          </Button>
        )}
        menu={{ label: "More actions for Logs", items: menuItems, triggerTestId: "logs-menu" }}
      />
      {loaded ? (
        <div className="logs__strip">
          <SeverityStrip
            bins={bins}
            start={loaded.start}
            end={loaded.end}
            now={follow ? loaded.end : undefined}
            range={range}
            onRange={setRange}
            onBinActivate={(index) => showBin(index)}
            label={`${plural(scoped.length, "event")} over ${formatTimeRange(loaded)}, ${plural(levelCounts(scoped).warn, "warning")}, ${plural(levelCounts(scoped).error, "error")}. Drag to pick a time range.`}
            testIdPrefix="logs"
          />
        </div>
      ) : null}
      <FilterBar
        className="logs__toolbar"
        ariaLabel="Filter logs"
        search={{ label: "Search events and text", value: query, onChange: setQuery, placeholder: "Search events and text", testId: "logs-search" }}
        chips={[
          { label: "All", count: inRange.length, pressed: level === "all", onToggle: () => setLevel("all"), testId: "logs-level:all" },
          { label: "Info", count: counts.info, pressed: level === "info", onToggle: (on) => setLevel(on ? "info" : "all"), testId: "logs-level:info" },
          { label: "Warnings", count: counts.warn, tone: "warn", pressed: level === "warn", onToggle: (on) => setLevel(on ? "warn" : "all"), testId: "logs-level:warn" },
          { label: "Errors", count: counts.error, tone: "crit", pressed: level === "error", onToggle: (on) => setLevel(on ? "error" : "all"), testId: "logs-level:error" },
        ]}
        modified={filtered}
        onClear={clearFilters}
        clearLabel="Clear filters"
      >
        {range ? (
          <span className="logs__range" data-testid="logs-range">
            <StatusPill tone="live" variant="outline">{formatTimeRange(range)}</StatusPill>
            <IconButton size="sm" glyph="close" label="Clear the time range" onClick={() => setRange(null)} />
          </span>
        ) : null}
        <SelectField
          label="Agent"
          showLabel={false}
          variant="chip"
          className="logs__agent"
          value={agent}
          onChange={(value) => setAgent(value)}
          testId="logs-agent-filter"
          options={[
            { value: "", label: "All agents" },
            ...agents.map(([key, count]) => ({ value: key, label: `${nameOf(key)} (${count})` })),
          ]}
        />
      </FilterBar>
      <div className="logs__body">
        <div className="logs__stream" ref={streamRef} role="list" aria-label="Events, newest first">
          {follow ? null : (
            <NewItemsPill count={newWhilePaused} onClick={() => setPaused(null)} testId="logs-new" />
          )}
          {items.length > 0 ? (
            <>
              <div className="logs__pad" style={{ height: virtual.padTop }} aria-hidden="true" />
              {items.slice(virtual.start, virtual.end).map((item) => (
                <div role="listitem" key={item.key} className="logs__item">
                  {item.kind === "row" ? renderRow(item) : renderGroup(item)}
                </div>
              ))}
              <div className="logs__pad" style={{ height: virtual.padBottom }} aria-hidden="true" />
            </>
          ) : filtered ? (
            <EmptyState
              kind="filtered"
              illustration="search"
              title="No matching events"
              body="Nothing in the log matches these filters."
              action={<Button size="sm" onClick={clearFilters}>Clear filters</Button>}
            />
          ) : (
            <EmptyState
              illustration="timeline"
              title="No events yet"
              body="Events from agents and the console appear here as they happen."
            />
          )}
        </div>
      </div>
    </PanelShell>
  );
}
