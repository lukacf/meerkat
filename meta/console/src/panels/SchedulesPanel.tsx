import React from "react";

import type { ConsoleAgent } from "../types";
import { agentDisplayTitle } from "../lib/agents";
import { isAgentRetired } from "../lib/agent-status";
import {
  CONSOLE_SCHEDULE_COMMANDS,
  consoleScheduleEditing,
  describeSchedulePhase,
  describeScheduleSender,
  describeScheduleTrigger,
  emptyScheduleDraft,
  formatScheduleTime,
  parseConsoleScheduleList,
  scheduleCreateParams,
  scheduleDraftFromSchedule,
  scheduleErrorMessage,
  scheduleIsGone,
  scheduleUpdateParams,
  viewerTimeZone,
  type ConsoleSchedule,
  type ConsoleScheduleCommand,
  type ConsoleScheduleDraft,
  type ConsoleScheduleEditing,
  type ConsoleScheduleOffer,
} from "../lib/schedules";
import {
  SCHEDULE_RUN_WINDOW_MS,
  countRunsBetween,
  formatRunTime,
  nextPlannedRun,
  planScheduleRuns,
  runsPerHour,
  scheduleKindWords,
  shortIntervalWords,
  type ScheduleKind,
  type ScheduleRunPlan,
} from "../lib/schedule-runs";
import { ScheduleForm, toggleDraftWeekday, useMeasuredWidth } from "./SchedulesSection";
import {
  Agenda,
  type AgendaBand,
  type AgendaRow,
  type AgendaRun,
  type AgendaShape,
} from "../../../packages/console-components/src/viz/charts/agenda";
import {
  AgentName,
  Button,
  Count,
  Countdown,
  DetailDrawer,
  DrawerLayout,
  DrawerSection,
  EmptyState,
  EntityRow,
  FigureTile,
  GlanceStrip,
  HeatStrip,
  IconButton,
  KitGlyph,
  LoadingState,
  Notice,
  PanelHeader,
  PanelShell,
  SegmentedControl,
  StatusPill,
  StatusRow,
  formatCountdown,
  plural,
  standardPanelMenu,
  useAgentDirectory,
  useNow,
  usePanelDensity,
  usePanelPreference,
  useShowTechnicalIds,
  type SummaryFact,
} from "../../../packages/console-components/src/panel-kit";

/** A list read is fresh for this long: refreshes inside it read nothing. */
const LIST_CACHE_MS = 15_000;
/** At most this many agents' lists are read at once. */
const LIST_CONCURRENCY = 4;
const DAY_MS = 24 * 3_600_000;
/** A repeat with more runs than this in the agenda's window is drawn as a band. */
const BAND_THRESHOLD = 48;

type SchedulesView = "agenda" | "week" | "list";
type AgendaRange = "day" | "week";
type PhaseFilter = "all" | "active" | "paused";

const VIEWS: readonly SchedulesView[] = ["agenda", "week", "list"];
const RANGES: readonly AgendaRange[] = ["day", "week"];

export interface SchedulesPanelProps {
  /** The agents the viewer sees (the sidebar's agents); retired ones are skipped. */
  agents: ConsoleAgent[];
  /** What the gateway offers: without the list, Schedules is not set up. */
  offer: ConsoleScheduleOffer;
  readOnly: boolean;
  accessEnforcing: boolean;
  /** Runs one schedule command for an agent; rejects with the console's typed error. */
  run: (identity: string, command: ConsoleScheduleCommand, params: Record<string, unknown>) => Promise<unknown>;
  /** Changes when the pane refreshes: stale lists are read again. */
  refreshToken?: unknown;
  now?: () => Date;
  timezone?: string;
  formatTime?: (iso: string) => string;
}

/** One agent's list as the panel last read it. */
interface AgentSchedules {
  status: "ready" | "refused" | "failed";
  schedules: ConsoleSchedule[];
  error: string | null;
  /** When the list was last read (ms), or tried. */
  at: number;
  /** When it was last read successfully. */
  okAt: number | null;
}

type Drawer =
  | { kind: "detail"; identity: string; scheduleId: string }
  | { kind: "form"; identity: string; schedule: ConsoleSchedule | null; draft: ConsoleScheduleDraft; error: string | null };

function rpcKind(error: unknown): string | null {
  const rpc = (error as { rpcError?: { data?: { kind?: unknown } } } | null)?.rpcError;
  return typeof rpc?.data?.kind === "string" ? rpc.data.kind : null;
}

/** A list the viewer may not read is a refusal, not a failure. */
function isRefusal(error: unknown): boolean {
  const kind = rpcKind(error);
  return kind === "access_denied" || kind === "unknown_identity";
}

const AGENDA_SHAPE: Record<ScheduleKind, AgendaShape> = {
  once: "square",
  interval: "tick",
  calendar: "diamond",
  unknown: "square",
};

/** The kind's mark beside a schedule's name (the agenda's shapes). */
function ScheduleKindMark({ kind, paused }: { kind: ScheduleKind; paused?: boolean }) {
  const shape = AGENDA_SHAPE[kind];
  return (
    <svg className="schedules__kind" width={12} height={12} viewBox="0 0 12 12" aria-hidden="true" focusable="false">
      {shape === "tick" ? (
        <rect className="cc-dots__mark cc-agenda__mark" data-paused={paused ? "true" : undefined} data-shape="tick" x={5.25} y={1} width={1.5} height={10} />
      ) : shape === "diamond" ? (
        <path className="cc-dots__mark cc-agenda__mark" data-paused={paused ? "true" : undefined} data-shape="diamond" d="M6 1 L11 6 L6 11 L1 6 Z" />
      ) : (
        <rect className="cc-dots__mark cc-agenda__mark" data-paused={paused ? "true" : undefined} data-shape="square" x={2} y={2} width={8} height={8} />
      )}
    </svg>
  );
}

/** The agenda's legend: what each mark means. */
function AgendaLegend() {
  const item = (mark: React.ReactNode, words: string) => (
    <span className="schedules__legend-item">
      {mark}
      {words}
    </span>
  );
  return (
    <div className="schedules__legend" aria-hidden="true" data-testid="schedules-legend">
      {item(<ScheduleKindMark kind="once" />, "Once")}
      {item(<ScheduleKindMark kind="interval" />, "Repeating")}
      {item(<ScheduleKindMark kind="calendar" />, "Calendar")}
      {item(
        <svg className="schedules__kind" width={12} height={12} viewBox="0 0 12 12" focusable="false">
          <rect className="cc-dots__mark is-live" data-shape="square" x={2} y={2} width={8} height={8} />
        </svg>,
        "Next run",
      )}
      {item(<ScheduleKindMark kind="calendar" paused />, "Paused")}
    </div>
  );
}

/**
 * Schedules (spec B10): what runs automatically, and when. It reads every
 * visible agent's schedules (four agents at a time, each list fresh for 15
 * seconds; an agent whose list the viewer may not read is counted, not an
 * error), plans their runs in the browser, and shows them as an agenda by
 * agent on a time axis, a week of runs per hour, or a list grouped by agent
 * with the actions the viewer may take. The form opens in the drawer.
 */
export function SchedulesPanel({
  agents,
  offer,
  readOnly,
  accessEnforcing,
  run,
  refreshToken,
  now,
  timezone,
  formatTime = formatScheduleTime,
}: SchedulesPanelProps): React.JSX.Element {
  const directory = useAgentDirectory();
  const sharedNow = useNow();
  const nowMs = now ? now().getTime() : sharedNow;
  const nowMinute = Math.floor(nowMs / 60_000) * 60_000;
  const [view, setView] = usePanelPreference<SchedulesView>("meta-console-schedules-view", "agenda", VIEWS);
  const [range, setRange] = usePanelPreference<AgendaRange>("meta-console-schedules-range", "day", RANGES);
  const [density, setDensity] = usePanelDensity("schedules");
  const [showIds, setShowIds] = useShowTechnicalIds("schedules");
  const [entries, setEntries] = React.useState<Record<string, AgentSchedules>>({});
  const [inFlight, setInFlight] = React.useState(0);
  const [drawer, setDrawer] = React.useState<Drawer | null>(null);
  const [busy, setBusy] = React.useState<string | null>(null);
  const [actionError, setActionError] = React.useState<string | null>(null);
  const [confirmingDelete, setConfirmingDelete] = React.useState<string | null>(null);
  const [phaseFilter, setPhaseFilter] = React.useState<PhaseFilter>("all");
  const [weekRef, weekWidth] = useMeasuredWidth<HTMLDivElement>(640);

  const runRef = React.useRef(run);
  runRef.current = run;
  const nowRef = React.useRef(now);
  nowRef.current = now;
  const entriesRef = React.useRef(entries);
  entriesRef.current = entries;
  const loading = React.useRef(new Set<string>());
  const alive = React.useRef(true);
  React.useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const visible = React.useMemo(
    () => agents.filter((agent) => !isAgentRetired(agent) && Boolean(agent.identity || agent.member_id)),
    [agents],
  );
  const identities = React.useMemo(() => visible.map((agent) => agent.identity || agent.member_id), [visible]);
  const identitiesKey = identities.join("\n");
  const agentOf = React.useCallback(
    (identity: string) => visible.find((agent) => (agent.identity || agent.member_id) === identity),
    [visible],
  );
  const nameOf = (identity: string) => {
    const agent = agentOf(identity);
    return agent ? agentDisplayTitle(agent) : directory.lookup(identity)?.label || identity;
  };
  const editingOf = (identity: string): ConsoleScheduleEditing =>
    consoleScheduleEditing({ offer, readOnly, accessEnforcing, agent: agentOf(identity) });

  const clock = () => (nowRef.current ? nowRef.current().getTime() : Date.now());

  /** Reads the lists of `targets`, four at a time; fresh lists are kept
   * unless `force`. */
  const load = React.useCallback(async (targets: readonly string[], force: boolean) => {
    const started = clock();
    const due = targets.filter((identity) => {
      if (loading.current.has(identity)) return false;
      const entry = entriesRef.current[identity];
      return force || !entry || started - entry.at >= LIST_CACHE_MS;
    });
    if (!due.length) return;
    for (const identity of due) loading.current.add(identity);
    setInFlight((count) => count + due.length);
    let next = 0;
    const worker = async () => {
      while (next < due.length) {
        const identity = due[next];
        next += 1;
        let entry: AgentSchedules;
        try {
          const result = await runRef.current(identity, CONSOLE_SCHEDULE_COMMANDS.list, {});
          const at = clock();
          entry = { status: "ready", schedules: parseConsoleScheduleList(result), error: null, at, okAt: at };
        } catch (error) {
          const previous = entriesRef.current[identity];
          const refused = isRefusal(error);
          entry = {
            status: refused ? "refused" : "failed",
            // A refused list is gone; a failed read keeps what was shown.
            schedules: refused ? [] : previous?.schedules ?? [],
            error: scheduleErrorMessage(error, CONSOLE_SCHEDULE_COMMANDS.list),
            at: clock(),
            okAt: refused ? null : previous?.okAt ?? null,
          };
        }
        loading.current.delete(identity);
        if (!alive.current) return;
        setEntries((current) => ({ ...current, [identity]: entry }));
        setInFlight((count) => Math.max(0, count - 1));
      }
    };
    await Promise.all(Array.from({ length: Math.min(LIST_CONCURRENCY, due.length) }, worker));
    // `clock` reads a ref; the loader stays one function.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  React.useEffect(() => {
    if (!offer.list) return;
    void load(identities, false);
    // identitiesKey stands for identities.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [offer.list, identitiesKey, refreshToken, load]);

  const refresh = () => {
    setActionError(null);
    void load(identities, true);
  };

  // ── Model ───────────────────────────────────────────────────────────
  const shown = identities.filter((identity) => entries[identity]);
  const schedules = React.useMemo(
    () => identities.flatMap((identity) => entries[identity]?.schedules ?? []),
    // identitiesKey stands for identities.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [entries, identitiesKey],
  );
  const plans = React.useMemo(
    () => schedules.map((schedule) => planScheduleRuns(schedule, nowMinute, SCHEDULE_RUN_WINDOW_MS)),
    [schedules, nowMinute],
  );
  const filtered = plans.filter((plan) =>
    phaseFilter === "all" ? true : phaseFilter === "active" ? plan.schedule.phase === "active" : plan.schedule.phase !== "active",
  );
  const total = schedules.length;
  const active = schedules.filter((schedule) => schedule.phase === "active").length;
  const paused = total - active;
  const next = nextPlannedRun(plans);
  const nextDay = countRunsBetween(plans, nowMinute, nowMinute + DAY_MS);
  const refused = shown.filter((identity) => entries[identity]?.status === "refused").length;
  const failed = shown.filter((identity) => entries[identity]?.status === "failed");
  const updatedAt = shown.reduce<number | null>((latest, identity) => {
    const at = entries[identity]?.okAt ?? null;
    return at !== null && (latest === null || at > latest) ? at : latest;
  }, null);
  const firstLoad = offer.list && shown.length < identities.length && total === 0 && inFlight > 0;
  const allFailed = offer.list && identities.length > 0 && shown.length === identities.length
    && failed.length > 0 && failed.length + refused === identities.length && total === 0;
  const changeable = identities.filter((identity) => editingOf(identity).canChange);
  const canCreate = changeable.length > 0;
  const planOf = (scheduleId: string) => plans.find((plan) => plan.schedule.scheduleId === scheduleId);

  // ── Writes ──────────────────────────────────────────────────────────
  async function write(identity: string, command: ConsoleScheduleCommand, params: Record<string, unknown>, key: string): Promise<boolean> {
    setBusy(key);
    setActionError(null);
    let done = false;
    try {
      await runRef.current(identity, command, params);
      done = true;
    } catch (error) {
      if (alive.current) {
        const message = scheduleErrorMessage(error, command);
        setActionError(message);
        setDrawer((current) => (current?.kind === "form" ? { ...current, error: message } : current));
        if (scheduleIsGone(error)) {
          setConfirmingDelete(null);
          setDrawer(null);
        }
      }
    }
    if (alive.current) setBusy(null);
    await load([identity], true);
    return done;
  }

  function openNew() {
    setActionError(null);
    setConfirmingDelete(null);
    const identity = changeable[0];
    if (!identity) return;
    setDrawer({
      kind: "form",
      identity,
      schedule: null,
      draft: emptyScheduleDraft(new Date(clock()), timezone ?? viewerTimeZone()),
      error: null,
    });
  }

  function openEdit(schedule: ConsoleSchedule) {
    setActionError(null);
    setConfirmingDelete(null);
    setDrawer({
      kind: "form",
      identity: schedule.identity,
      schedule,
      draft: scheduleDraftFromSchedule(schedule, new Date(clock()), timezone ?? viewerTimeZone()),
      error: null,
    });
  }

  function openDetail(schedule: ConsoleSchedule) {
    setConfirmingDelete(null);
    setDrawer({ kind: "detail", identity: schedule.identity, scheduleId: schedule.scheduleId });
  }

  function patchForm(patch: Partial<Extract<Drawer, { kind: "form" }>>) {
    setDrawer((current) => (current?.kind === "form" ? { ...current, ...patch } : current));
  }

  function submit(event: React.FormEvent) {
    event.preventDefault();
    if (drawer?.kind !== "form" || busy) return;
    const form = drawer;
    const at = new Date(clock());
    const request = form.schedule ? scheduleUpdateParams(form.schedule, form.draft, at) : scheduleCreateParams(form.draft, at);
    if ("error" in request) {
      patchForm({ error: request.error });
      return;
    }
    const command = form.schedule ? CONSOLE_SCHEDULE_COMMANDS.update : CONSOLE_SCHEDULE_COMMANDS.create;
    void write(form.identity, command, request.params, "form").then((done) => {
      if (done && alive.current) setDrawer(null);
    });
  }

  function runAction(schedule: ConsoleSchedule, command: ConsoleScheduleCommand) {
    if (busy) return;
    void write(schedule.identity, command, { schedule_id: schedule.scheduleId }, `${schedule.scheduleId}:${command}`).then((done) => {
      if (done && alive.current && command === CONSOLE_SCHEDULE_COMMANDS.delete) {
        setConfirmingDelete(null);
        setDrawer((current) =>
          current && "scheduleId" in current && current.scheduleId === schedule.scheduleId
            ? null
            : current?.kind === "form" && current.schedule?.scheduleId === schedule.scheduleId
              ? null
              : current,
        );
      }
    });
  }

  // ── Header ──────────────────────────────────────────────────────────
  const summary: Array<SummaryFact | null> = offer.list
    ? [
        { key: "total", text: plural(total, "schedule") },
        { key: "active", text: `${active} active` },
        next
          ? {
              key: "next",
              text: `next: ${next.plan.schedule.name || "Untitled schedule"} ${formatCountdown(next.at - nowMs)}`,
              announce: false,
            }
          : null,
        refused
          ? {
              key: "refused",
              text: `${plural(refused, "agent")} not readable`,
              tone: "locked",
              title: "You may not view these agents' schedules.",
            }
          : null,
      ]
    : [];
  const viewOptions = [
    { id: "agenda" as const, label: "Agenda", glyph: "lanes" as const, testId: "schedules-view:agenda" },
    { id: "week" as const, label: "Week", glyph: "calendar" as const, testId: "schedules-view:week" },
    { id: "list" as const, label: "List", glyph: "list" as const, testId: "schedules-view:list" },
  ];
  const header = (
    <PanelHeader
      title="Schedules"
      summary={summary}
      summaryTestId="schedules-summary"
      freshness={offer.list ? { at: updatedAt ?? undefined, updating: inFlight > 0, testId: "schedules-freshness" } : undefined}
      refreshing={inFlight > 0}
      view={offer.list ? { ariaLabel: "Schedules view", options: viewOptions, value: view, onChange: setView, testId: "schedules-view" } : undefined}
      tools={
        offer.list ? (
          <>
            {canCreate ? <IconButton label="New schedule" glyph="plus" onClick={openNew} data-testid="schedules-new-tool" /> : null}
            <IconButton label="Refresh schedules" glyph="refresh" onClick={refresh} data-testid="schedules-refresh" />
          </>
        ) : undefined
      }
      menu={{
        label: "More actions for Schedules",
        triggerTestId: "schedules-menu",
        items: standardPanelMenu({
          viewOptions: canCreate ? [{ kind: "item", id: "new", label: "New schedule", glyph: "plus", onSelect: openNew, testId: "schedules-menu:new" }] : [],
          density,
          onDensityChange: setDensity,
          showTechnicalIds: showIds,
          onShowTechnicalIdsChange: setShowIds,
          getCopyData: () => ({ schedules: schedules.map((schedule) => ({ ...schedule })) }),
          onRefresh: offer.list ? refresh : undefined,
          testIdPrefix: "schedules",
        }),
      }}
    />
  );

  if (!offer.list) {
    return (
      <PanelShell className="schedules" testId="schedules-panel" density={density} showTechnicalIds={showIds}>
        {header}
        <EmptyState
          kind="not-configured"
          illustration="not-configured"
          title="Schedules aren't set up on this console."
          body="Schedules send a message to an agent at set times: once, on a repeat, or on a calendar. This console's gateway has no schedule host, so nothing is planned here. Attach a schedule host to the gateway to plan sends from the console."
          testId="schedules-not-set-up"
        />
      </PanelShell>
    );
  }

  // ── Glance ──────────────────────────────────────────────────────────
  const glance = (
    <GlanceStrip ariaLabel="Schedules at a glance" testId="schedules-glance">
      <FigureTile
        label="Active"
        value={active}
        of={total || undefined}
        tone={active ? "ok" : undefined}
        onActivate={() => setPhaseFilter((current) => (current === "active" ? "all" : "active"))}
        pressed={phaseFilter === "active"}
        loading={firstLoad}
        testId="schedules-glance:active"
      />
      <FigureTile
        label="Next run"
        value={next ? formatCountdown(next.at - nowMs) : "None"}
        foot={next ? `${next.plan.schedule.name || "Untitled schedule"} · ${nameOf(next.plan.schedule.identity)}` : "No run planned"}
        title={next ? formatRunTime(next.at, next.plan.schedule) : undefined}
        loading={firstLoad}
        testId="schedules-glance:next"
      />
      <FigureTile
        label="Next 24 h"
        value={nextDay}
        unit={nextDay === 1 ? "run" : "runs"}
        loading={firstLoad}
        testId="schedules-glance:day"
      />
      <FigureTile
        label="Paused"
        value={paused}
        tone={paused ? "warn" : undefined}
        onActivate={() => setPhaseFilter((current) => (current === "paused" ? "all" : "paused"))}
        pressed={phaseFilter === "paused"}
        loading={firstLoad}
        testId="schedules-glance:paused"
      />
    </GlanceStrip>
  );

  // ── Agenda ──────────────────────────────────────────────────────────
  const windowMs = range === "week" ? SCHEDULE_RUN_WINDOW_MS : DAY_MS;
  const windowEnd = nowMinute + windowMs;
  const byAgent = new Map<string, ScheduleRunPlan[]>();
  for (const plan of filtered) {
    const list = byAgent.get(plan.schedule.identity) ?? [];
    list.push(plan);
    byAgent.set(plan.schedule.identity, list);
  }
  const agendaRows: AgendaRow[] = [...byAgent.entries()]
    .map(([identity, agentPlans]) => {
      const runs: AgendaRun[] = [];
      const bands: AgendaBand[] = [];
      for (const plan of agentPlans) {
        const { schedule } = plan;
        const name = schedule.name || "Untitled schedule";
        const isPaused = schedule.phase !== "active";
        const inWindow = plan.runs.filter((t) => t <= windowEnd);
        const trigger = schedule.trigger;
        if (trigger?.type === "interval" && trigger.every_seconds * 1000 < windowMs / BAND_THRESHOLD && inWindow.length) {
          const step = trigger.every_seconds * 1000;
          const endAt = trigger.end_at_utc ? Date.parse(trigger.end_at_utc) : Number.NaN;
          const to = Number.isFinite(endAt) ? Math.min(windowEnd, endAt) : windowEnd;
          const count = Math.floor((to - inWindow[0]) / step) + 1;
          const words = shortIntervalWords(trigger.every_seconds);
          bands.push({
            id: `${schedule.scheduleId}:band`,
            from: inWindow[0],
            to,
            words,
            label: `${name} · ${words}, ${plural(count, "run")} in the ${range === "week" ? "next 7 days" : "next 24 hours"}${isPaused ? " (paused)" : ""}`,
            paused: isPaused,
            data: schedule,
          });
          continue;
        }
        inWindow.forEach((t, index) => {
          runs.push({
            id: `${schedule.scheduleId}:${index}`,
            t,
            shape: AGENDA_SHAPE[plan.kind],
            next: !isPaused && plan.next === t,
            paused: isPaused,
            label: `${name} · ${formatRunTime(t, schedule)}${isPaused ? " (paused)" : ""}`,
            data: schedule,
          });
        });
      }
      const firstNext = agentPlans.reduce<number>((min, plan) => (plan.next !== null && plan.next < min ? plan.next : min), Infinity);
      return { identity, row: { key: identity, label: <AgentName id={identity} link avatar={18} />, labelText: nameOf(identity), runs, bands }, firstNext };
    })
    .sort((a, b) => a.firstNext - b.firstNext || a.row.labelText.localeCompare(b.row.labelText))
    .map((entry) => entry.row);
  const agendaRuns = agendaRows.reduce((sum, row) => sum + row.runs.length, 0);
  const rangeWords = range === "week" ? "the next 7 days" : "the next 24 hours";
  const agendaLabel = `${plural(agendaRuns, "planned run")} in ${rangeWords} across ${plural(agendaRows.length, "agent")}`
    + (next ? `; next: ${next.plan.schedule.name || "Untitled schedule"} ${formatCountdown(next.at - nowMs)}` : "");
  const selectedKey = drawer?.kind === "detail" ? drawer.scheduleId : null;

  const agenda = (
    <div className="schedules__agenda" data-testid="schedules-agenda-view">
      <div className="schedules__toolbar">
        <SegmentedControl
          ariaLabel="Agenda range"
          value={range}
          onChange={setRange}
          options={[
            { id: "day", label: "24 hours", testId: "schedules-range:day" },
            { id: "week", label: "7 days", testId: "schedules-range:week" },
          ]}
          className="schedules__range"
        />
        <AgendaLegend />
      </div>
      {agendaRows.length ? (
        <Agenda
          rows={agendaRows}
          start={nowMinute}
          end={windowEnd}
          label={agendaLabel}
          onSelect={(item) => openDetail(item.data as ConsoleSchedule)}
          selectedId={null}
          testIdPrefix="schedules-agenda-run"
          rowTestIdPrefix="schedules-agenda-row"
          testId="schedules-agenda"
          className={selectedKey ? "has-selection" : undefined}
        />
      ) : (
        <EmptyState
          inline
          illustration="timeline"
          title={phaseFilter === "all" ? `Nothing runs in ${rangeWords}.` : "No schedules match this filter."}
          body={range === "day" && phaseFilter === "all" ? "Switch to 7 days to see what runs later this week." : undefined}
          testId="schedules-agenda-empty"
        />
      )}
    </div>
  );

  // ── Week ────────────────────────────────────────────────────────────
  const weekRows = runsPerHour(filtered, nowMinute);
  let busiest: { label: string; hour: number; count: number } | null = null;
  for (const row of weekRows) {
    row.hours.forEach((count, hour) => {
      if (count > 0 && (!busiest || count > busiest.count)) busiest = { label: row.label, hour, count };
    });
  }
  const weekTotal = weekRows.reduce((sum, row) => sum + row.hours.reduce((a, b) => a + b, 0), 0);
  const pad = (value: number) => String(value).padStart(2, "0");
  const peak = busiest as { label: string; hour: number; count: number } | null;
  const weekLabel = `${plural(weekTotal, "planned run")} over the next 7 days`
    + (peak ? `, busiest ${peak.label} at ${pad(peak.hour)}:00 with ${plural(peak.count, "run")}` : "");
  const cell = Math.max(10, Math.min(26, Math.floor((weekWidth - 72 - 23 * 3) / 24)));
  const week = (
    <div className="schedules__week" ref={weekRef} data-testid="schedules-week-view">
      {weekTotal ? (
        <>
          <HeatStrip
            rows={weekRows.map((row) => ({ key: String(row.start), label: row.label, values: row.hours }))}
            columns={Array.from({ length: 24 }, (_, hour) => `${pad(hour)}:00`)}
            columnTick={(index) => (index % 3 === 0 ? pad(index) : null)}
            label={weekLabel}
            cell={cell}
            gap={3}
            rowLabelWidth={72}
            formatCell={(row, column, value) => `${row.label} · ${pad(column)}:00 · ${plural(value, "run")}`}
            testId="schedules-week"
          />
          <p className="schedules__caption">
            {weekLabel}. {phaseFilter === "paused" ? "Paused schedules plan no runs." : "Paused schedules are not counted."}
          </p>
        </>
      ) : (
        <EmptyState inline illustration="timeline" title="Nothing runs in the next 7 days." testId="schedules-week-empty" />
      )}
    </div>
  );

  // ── List ────────────────────────────────────────────────────────────
  const listGroups = [...byAgent.entries()].sort((a, b) => nameOf(a[0]).localeCompare(nameOf(b[0])));
  const list = (
    <div className="schedules__list" data-testid="schedules-list">
      {canCreate ? (
        <div className="schedules__toolbar">
          <Button size="sm" onClick={openNew} data-testid="schedules-new" icon={<KitGlyph name="plus" />}>
            New schedule
          </Button>
        </div>
      ) : null}
      {listGroups.map(([identity, agentPlans]) => {
        const editing = editingOf(identity);
        return (
          <section key={identity} className="schedules__group" data-testid={`schedules-group:${identity}`} aria-label={`${nameOf(identity)}'s schedules`}>
            <div className="schedules__group-head">
              <AgentName id={identity} link avatar={22} />
              <Count value={agentPlans.length} />
              {editing.note ? <span className="schedules__group-note">{editing.note}</span> : null}
            </div>
            {agentPlans.map((plan) => renderRow(plan, editing))}
          </section>
        );
      })}
    </div>
  );

  function renderRow(plan: ScheduleRunPlan, editing: ConsoleScheduleEditing) {
    const { schedule } = plan;
    const id = schedule.scheduleId;
    const name = schedule.name || "Untitled schedule";
    const isPaused = schedule.phase !== "active";
    const working = busy !== null;
    const sender = schedule.senderHidden ? null : describeScheduleSender(schedule.sender, (raw) => directory.lookup(raw)?.label);
    const actions = editing.canChange ? (
      <span className="schedules__actions">
        {confirmingDelete === id ? (
          <>
            <Button
              size="sm"
              variant="danger"
              aria-label={`Confirm deleting ${name}`}
              data-testid={`schedules-delete-confirm:${id}`}
              disabled={working}
              autoFocus
              onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.delete)}
            >
              Confirm delete
            </Button>
            <Button size="sm" variant="ghost" data-testid={`schedules-delete-cancel:${id}`} onClick={() => setConfirmingDelete(null)}>
              Keep
            </Button>
          </>
        ) : (
          <>
            <Button size="sm" variant="ghost" aria-label={`Edit ${name}`} data-testid={`schedules-edit:${id}`} disabled={working} onClick={() => openEdit(schedule)}>
              Edit
            </Button>
            {isPaused ? (
              <Button
                size="sm"
                variant="ghost"
                aria-label={`Resume ${name}`}
                title="Resuming makes you its sender"
                data-testid={`schedules-resume:${id}`}
                disabled={working}
                onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.resume)}
              >
                Resume
              </Button>
            ) : (
              <Button size="sm" variant="ghost" aria-label={`Pause ${name}`} data-testid={`schedules-pause:${id}`} disabled={working} onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.pause)}>
                Pause
              </Button>
            )}
            <Button size="sm" variant="ghost" className="schedules__delete" aria-label={`Delete ${name}`} data-testid={`schedules-delete:${id}`} disabled={working} onClick={() => setConfirmingDelete(id)}>
              Delete
            </Button>
          </>
        )}
      </span>
    ) : undefined;
    return (
      <EntityRow
        key={id}
        kind="rule"
        testId={`schedules-row:${id}`}
        dataAttributes={{ "data-phase": schedule.phase }}
        className={confirmingDelete === id ? "is-confirming" : undefined}
        mark={<ScheduleKindMark kind={plan.kind} paused={isPaused} />}
        title={name}
        subtitle={
          <>
            <span>{describeScheduleTrigger(schedule.trigger, formatTime)}</span>
            {sender ? <span title={schedule.sender || undefined}>{` · Sends as ${sender}`}</span> : null}
          </>
        }
        status={
          <StatusPill tone={isPaused ? "warn" : "ok"} variant="quiet" testId={`schedules-phase:${id}`}>
            {describeSchedulePhase(schedule.phase)}
          </StatusPill>
        }
        time={
          plan.next !== null ? (
            <Countdown to={plan.next} now={now ? nowMs : undefined} testId={`schedules-next:${id}`} />
          ) : (
            <span className="schedules__no-run">{isPaused ? "Paused" : "No run planned"}</span>
          )
        }
        actions={actions}
        selected={selectedKey === id}
        onActivate={() => openDetail(schedule)}
        activateLabel={`${name}, ${scheduleKindWords(plan.kind).toLowerCase()}, ${describeSchedulePhase(schedule.phase).toLowerCase()}`}
      />
    );
  }

  // ── Drawer ──────────────────────────────────────────────────────────
  let drawerNode: React.ReactNode = null;
  if (drawer?.kind === "form") {
    const form = drawer;
    const agentChoice = !form.schedule && changeable.length > 1 ? (
      <label className="console-schedules__field">
        Agent
        <span className="cc-select">
          <select
            className="cc-select__control"
            data-testid="schedule-form-agent"
            value={form.identity}
            onChange={(event) => patchForm({ identity: event.target.value, error: null })}
          >
            {changeable.map((identity) => (
              <option key={identity} value={identity}>
                {nameOf(identity)}
              </option>
            ))}
          </select>
          <KitGlyph name="chevron" className="cc-select__chevron" />
        </span>
      </label>
    ) : null;
    drawerNode = (
      <DetailDrawer
        title={form.schedule ? `Edit ${form.schedule.name || "schedule"}` : "New schedule"}
        subtitle={<>For <AgentName id={form.identity} /></>}
        onClose={() => setDrawer(null)}
        testId="schedules-drawer"
        closeTestId="schedules-drawer-close"
      >
        <ScheduleForm
          identity={form.identity}
          agentName={nameOf(form.identity)}
          schedule={form.schedule}
          draft={form.draft}
          error={form.error}
          busy={busy !== null}
          saving={busy === "form"}
          onChange={(patch) => patchForm({ draft: { ...form.draft, ...patch }, error: null })}
          onToggleWeekday={(day) => patchForm({ draft: toggleDraftWeekday(form.draft, day), error: null })}
          onSubmit={submit}
          onCancel={() => setDrawer(null)}
          formatTime={formatTime}
          lead={agentChoice}
          className="schedules__form"
        />
      </DetailDrawer>
    );
  } else if (drawer?.kind === "detail") {
    const plan = planOf(drawer.scheduleId);
    if (plan) {
      const { schedule } = plan;
      const name = schedule.name || "Untitled schedule";
      const isPaused = schedule.phase !== "active";
      const editing = editingOf(schedule.identity);
      const upcoming = plan.runs.slice(0, 5);
      drawerNode = (
        <DetailDrawer
          title={name}
          mark={<ScheduleKindMark kind={plan.kind} paused={isPaused} />}
          status={<StatusPill tone={isPaused ? "warn" : "ok"}>{describeSchedulePhase(schedule.phase)}</StatusPill>}
          subtitle={<>For <AgentName id={schedule.identity} link /></>}
          onClose={() => setDrawer(null)}
          testId={`schedules-detail:${schedule.scheduleId}`}
          closeTestId="schedules-drawer-close"
          actions={
            editing.canChange ? (
              <div className="cc-btn-row">
                <Button size="sm" variant="primary" onClick={() => openEdit(schedule)} data-testid="schedules-detail-edit">
                  Edit
                </Button>
                {isPaused ? (
                  <Button size="sm" onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.resume)} disabled={busy !== null} title="Resuming makes you its sender">
                    Resume
                  </Button>
                ) : (
                  <Button size="sm" onClick={() => runAction(schedule, CONSOLE_SCHEDULE_COMMANDS.pause)} disabled={busy !== null}>
                    Pause
                  </Button>
                )}
              </div>
            ) : editing.note ? (
              <p className="schedules__caption">{editing.note}</p>
            ) : null
          }
          technical={[
            { label: "Schedule", value: schedule.scheduleId },
            { label: "Agent", value: schedule.identity },
            schedule.revision !== null ? { label: "Revision", value: schedule.revision } : null,
            schedule.sender ? { label: "Sender", value: schedule.sender } : null,
            schedule.updatedAt ? { label: "Updated", value: schedule.updatedAt, plain: true } : null,
          ]}
        >
          <DrawerSection label="When">
            <p className="schedules__detail-text">{describeScheduleTrigger(schedule.trigger, formatTime)}</p>
            {plan.invalidZone ? <Notice tone="warn">This console cannot read the schedule's time zone, so its runs are not drawn.</Notice> : null}
          </DrawerSection>
          <DrawerSection label="Next runs" count={upcoming.length || null}>
            {upcoming.length ? (
              <ol className="schedules__runs">
                {upcoming.map((t, index) => (
                  <li key={t}>
                    <span>{formatRunTime(t, schedule)}</span>
                    {index === 0 && !isPaused ? <Countdown to={t} now={now ? nowMs : undefined} /> : null}
                  </li>
                ))}
              </ol>
            ) : (
              <p className="schedules__caption">{isPaused ? "Paused: nothing runs until it is resumed." : "No run in the next 7 days."}</p>
            )}
          </DrawerSection>
          <DrawerSection label="Message">
            <blockquote className="schedules__message">{schedule.content}</blockquote>
          </DrawerSection>
          {schedule.senderHidden ? null : (
            <DrawerSection label="Sends as">
              <p className="schedules__detail-text" title={schedule.sender || undefined}>
                {describeScheduleSender(schedule.sender, (raw) => directory.lookup(raw)?.label)}
              </p>
            </DrawerSection>
          )}
        </DetailDrawer>
      );
    }
  }

  // ── Body ────────────────────────────────────────────────────────────
  let main: React.ReactNode;
  if (firstLoad) {
    main = <LoadingState variant="timeline" label="Loading schedules" testId="schedules-loading" />;
  } else if (allFailed) {
    main = (
      <EmptyState
        kind="error"
        illustration="error"
        title="Couldn't load schedules"
        body={entries[failed[0]]?.error ?? undefined}
        action={<Button size="sm" onClick={refresh}>Retry</Button>}
        testId="schedules-error"
      />
    );
  } else if (total === 0 && inFlight === 0) {
    main = (
      <EmptyState
        illustration="timeline"
        title="No schedules yet"
        body={
          refused && refused === identities.length
            ? "You may not view any agent's schedules here."
            : "Schedules send a message to an agent at set times: once, on a repeat, or on a calendar. None of the agents you can see has one."
        }
        action={canCreate ? <Button size="sm" variant="primary" onClick={openNew}>New schedule</Button> : undefined}
        testId="schedules-empty"
      />
    );
  } else {
    main = view === "week" ? week : view === "list" ? list : agenda;
  }

  return (
    <PanelShell className="schedules" testId="schedules-panel" density={density} showTechnicalIds={showIds}>
      {header}
      {total > 0 || firstLoad ? glance : null}
      <div className="schedules__frame">
        <StatusRow
          mode="toast"
          error={failed.length && !allFailed ? `Couldn't refresh the schedules of ${plural(failed.length, "agent")}.` : null}
          errorTestId="schedules-panel-error"
          updatedAt={updatedAt ?? undefined}
          onRetry={() => void load(failed, true)}
        />
        {actionError && drawer?.kind !== "form" ? (
          <Notice tone="crit" className="schedules__notice" testId="schedules-action-error">
            {actionError}
          </Notice>
        ) : null}
        <DrawerLayout
          main={<div className="schedules__main">{main}</div>}
          drawer={drawerNode}
          open={Boolean(drawerNode)}
          onClose={() => setDrawer(null)}
          persistKey="schedules"
          backLabel="All schedules"
          backTestId="schedules-drawer-back"
          testId="schedules-layout"
        />
      </div>
    </PanelShell>
  );
}
