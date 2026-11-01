import React from "react";
import type { IdentityStatusRow, UnobservedFacts } from "@console-core";
import {
  AgentAvatar,
  Chip,
  EmptyState,
  EntityCard,
  FigureTile,
  GlanceStrip,
  HeatStrip,
  KeyValueList,
  PanelBody,
  PanelHeader,
  PanelSection,
  PanelShell,
  StatusPill,
  humanizeEnum,
  plural,
  standardPanelMenu,
  useAgentDirectory,
  usePanelDensity,
  useShowTechnicalIds,
  type SummaryFact,
  type Tone,
} from "../../../packages/console-components/src/panel-kit";
import { agentDisplayTitle } from "../../../packages/console-core/src/agent-title";
import { reachabilityWords } from "./AgentDetailsPanel";
import type { ConsoleAgent } from "../types";
import { agentHealth, type AgentHealth, type AgentHealthKind } from "../lib/agent-status";
import { useActivityLedger } from "../lib/activity-ledger";
import { HOUR_MS, agentActivity, healthReason, hourCeiling, hourLabel, seriesWords, type AgentActivity } from "../lib/agent-activity";
import { useElementWidth } from "../lib/use-element-width";

/** The team facts Health reads besides its agents (`health_overview.live_snapshot`). */
export interface HealthTeamSnapshot {
  /** Whether the team runs; null when nothing observed it. */
  running?: boolean | null;
  loadedModules?: unknown[];
  loadedModuleCount?: number;
  unobserved?: UnobservedFacts;
}

/** The order tiles take: the worst first. */
const KIND_ORDER: Readonly<Record<AgentHealthKind, number>> = {
  broken: 0,
  stalled: 1,
  needs_repair: 2,
  degraded: 3,
  unobserved: 4,
  working: 5,
  healthy: 6,
  retiring: 7,
  retired: 8,
};

/** How many healthy tiles show before "Show all". */
const HEALTHY_TILES_SHOWN = 24;
/** Rows of the activity grid: the busiest agents. */
const HEAT_ROWS_SHOWN = 24;
/** Room for the agents' names at the left of the activity grid. */
const HEAT_LABEL_WIDTH = 140;

type HealthFilter = "healthy" | "working" | "degraded" | "stalled" | "repair";

const FILTER_KINDS: Readonly<Record<HealthFilter, readonly AgentHealthKind[]>> = {
  healthy: ["healthy"],
  working: ["working"],
  degraded: ["degraded"],
  stalled: ["stalled", "broken"],
  repair: ["needs_repair"],
};

interface HealthEntry {
  row: IdentityStatusRow;
  identity: string;
  name: string;
  health: AgentHealth;
  activity: AgentActivity;
  reason: string;
}

/** The status fields of a health row, with the member state the console
 * shows (the repair hold reads "needs repair") and, from the roster, the
 * response phase that says a busy agent is working. */
function healthOf(row: IdentityStatusRow, label: string, agent: ConsoleAgent | undefined, activity: AgentActivity): AgentHealth {
  const state = label.trim().toLowerCase() === "needs repair" ? "needs_repair" : label;
  return agentHealth(
    {
      state,
      response_phase: agent?.response_phase,
      progress: row.progress ?? agent?.progress,
      labels: row.labels,
      unobserved: row.unobserved ?? agent?.unobserved,
    },
    { openRun: activity.openRun },
  );
}

/** A tile's second line: the role, and the reachability only where it is
 * not the usual "Can receive messages". */
function tileSubtitle(row: IdentityStatusRow): string {
  const role = row.role ? humanizeEnum(row.role) : "";
  const reach = row.addressability === "addressable" ? "" : reachabilityWords(row.addressability);
  return [role, reach].filter(Boolean).join(" · ");
}

/** "router · delivery": the loaded modules' names in words. */
function moduleNames(modules: unknown[] | undefined): string[] {
  return (modules || [])
    .map((module) => {
      if (typeof module === "string") return module;
      if (module && typeof module === "object") {
        const record = module as Record<string, unknown>;
        const name = record.name ?? record.id ?? record.module;
        return typeof name === "string" ? name : "";
      }
      return "";
    })
    .filter(Boolean);
}

function teamFact(team: HealthTeamSnapshot | null | undefined): SummaryFact | null {
  if (!team) return null;
  if (team.running === true) return { key: "team", text: "team running", tone: "ok", testId: "health-summary:team" };
  if (team.running === false) return { key: "team", text: "team stopped", tone: "warn", testId: "health-summary:team" };
  if (team.running === null) return { key: "team", text: "team not observed", tone: "locked", testId: "health-summary:team" };
  return null;
}

/**
 * Health (console v2 spec B11): is every agent healthy, and if not, which
 * and since when. A glance strip of counts by health, one tile per agent
 * (the worst first: its status in words, why, a day's activity), the team's
 * activity by hour, and the team's own state and modules. A tile opens the
 * agent's details beside Health.
 */
export function HealthPanel({
  identities,
  stateLabel,
  onOpen,
  canOpen,
  agents,
  team,
  now,
}: {
  identities: IdentityStatusRow[];
  stateLabel: (row: IdentityStatusRow) => string;
  onOpen?: (identity: string) => void;
  /** Whether a row's agent has details to open; rows that cannot open are
   * not buttons. */
  canOpen?: (identity: string) => boolean;
  /** The roster's agents: their response phase tells a busy agent apart. */
  agents?: ConsoleAgent[];
  team?: HealthTeamSnapshot | null;
  /** A fixed clock (tests, the gallery). */
  now?: () => number;
}): React.JSX.Element {
  const directory = useAgentDirectory();
  const ledger = useActivityLedger({ tickMs: 30_000 });
  const nowMs = now ? now() : ledger.nowMs;
  const [density, setDensity] = usePanelDensity("health");
  const [showIds, setShowIds] = useShowTechnicalIds("health");
  const [filter, setFilter] = React.useState<HealthFilter | null>(null);
  const [showAllHealthy, setShowAllHealthy] = React.useState(false);

  // Named the way every other surface names the agent; the identity is the tooltip.
  const titleOf = (row: IdentityStatusRow) => directory.lookup(row.identity)?.label
    || agentDisplayTitle({ label: row.display_name || "", identity: row.identity, role: row.role });

  const entries = React.useMemo(() => {
    const byKey = new Map<string, ConsoleAgent>();
    for (const agent of agents || []) {
      for (const key of [agent.identity, agent.member_id, agent.agent_id]) if (key) byKey.set(key, agent);
    }
    return identities
      .map((row): HealthEntry => {
        const agent = byKey.get(row.identity);
        const activity = agentActivity(ledger, {
          identity: row.identity,
          member_id: agent?.member_id || row.identity,
          agent_id: agent?.agent_id || row.identity,
          last_activity_ms: row.last_activity_ms ?? agent?.last_activity_ms,
          progress: row.progress ?? agent?.progress,
        }, nowMs);
        const health = healthOf(row, stateLabel(row), agent, activity);
        const progress = row.progress ?? agent?.progress;
        return {
          row,
          identity: row.identity,
          name: titleOf(row),
          health,
          activity,
          reason: healthReason(health, {
            progress,
            unobserved: row.unobserved,
            lastActivityMs: activity.lastActivityMs,
            workingSinceMs: activity.live.openTool?.startMs ?? activity.live.openRun?.startMs ?? null,
            nowMs,
          }),
        };
      })
      .sort((left, right) => KIND_ORDER[left.health.kind] - KIND_ORDER[right.health.kind] || left.name.localeCompare(right.name));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [identities, agents, ledger.version, nowMs, directory, stateLabel]);

  const count = (...kinds: AgentHealthKind[]) => entries.filter((entry) => kinds.includes(entry.health.kind)).length;
  const healthy = count("healthy");
  const working = count("working");
  const degraded = count("degraded");
  const stalled = count("stalled");
  const broken = count("broken");
  const repair = count("needs_repair");
  const unobserved = count("unobserved");
  const retired = count("retired", "retiring");
  const members = entries.length - retired;
  const modules = moduleNames(team?.loadedModules);
  const moduleCount = team?.loadedModuleCount ?? (team?.loadedModules ? modules.length : undefined);

  const summary: Array<SummaryFact | null> = [
    { key: "agents", text: plural(entries.length, "agent"), testId: "health-summary:agents" },
    healthy ? { key: "healthy", text: `${healthy} healthy`, testId: "health-summary:healthy" } : null,
    working ? { key: "working", text: `${working} working`, tone: "live", testId: "health-summary:working" } : null,
    degraded ? { key: "degraded", text: `${degraded} degraded`, tone: "warn", testId: "health-summary:degraded" } : null,
    stalled ? { key: "stalled", text: `${stalled} stalled`, tone: "crit", testId: "health-summary:stalled" } : null,
    broken ? { key: "broken", text: `${broken} broken`, tone: "crit", testId: "health-summary:broken" } : null,
    repair ? { key: "repair", text: `${repair} need${repair === 1 ? "s" : ""} repair`, tone: "warn", testId: "health-summary:repair" } : null,
    unobserved ? { key: "unobserved", text: `${unobserved} not observed`, tone: "locked", testId: "health-summary:unobserved" } : null,
    teamFact(team),
    moduleCount !== undefined ? { key: "modules", text: plural(moduleCount, "module"), testId: "health-summary:modules", announce: false } : null,
  ];

  const shown = filter ? entries.filter((entry) => FILTER_KINDS[filter].includes(entry.health.kind)) : entries;
  const problems = shown.filter((entry) => KIND_ORDER[entry.health.kind] < KIND_ORDER.working);
  const fine = shown.filter((entry) => KIND_ORDER[entry.health.kind] >= KIND_ORDER.working);
  const fineShown = showAllHealthy || filter ? fine : fine.slice(0, HEALTHY_TILES_SHOWN);
  const tiles = [...problems, ...fineShown];

  const heatRef = React.useRef<HTMLDivElement | null>(null);
  const heatWidth = useElementWidth(heatRef);
  // Cells grow with the pane, 8 to 16 px, so the grid reads at every width.
  const heatCell = heatWidth > 0
    ? Math.max(8, Math.min(16, Math.floor((heatWidth - HEAT_LABEL_WIDTH) / 24) - 3))
    : 10;
  const endMs = hourCeiling(nowMs);
  const heat = React.useMemo(() => {
    const ranked = entries
      .filter((entry) => entry.health.kind !== "retired")
      .map((entry) => ({ entry, total: entry.activity.hourly.reduce((sum, value) => sum + value, 0) }))
      .sort((left, right) => right.total - left.total || left.entry.name.localeCompare(right.entry.name));
    return {
      rows: ranked.slice(0, HEAT_ROWS_SHOWN).map(({ entry }) => ({ key: entry.identity, label: entry.name, values: entry.activity.hourly })),
      hidden: Math.max(0, ranked.length - HEAT_ROWS_SHOWN),
      busiest: ranked[0],
    };
  }, [entries]);
  const heatColumns = React.useMemo(
    () => Array.from({ length: 24 }, (_, index) => hourLabel(endMs - (24 - index) * HOUR_MS)),
    [endMs],
  );

  const toggle = (next: HealthFilter) => setFilter((current) => (current === next ? null : next));
  const figure = (key: HealthFilter, label: string, value: number, tone: Tone | undefined, foot?: React.ReactNode) => (
    <FigureTile
      label={label}
      value={value}
      tone={value ? tone : undefined}
      foot={foot}
      onActivate={() => toggle(key)}
      pressed={filter === key}
      title={`Show only the ${label.toLowerCase()} agents`}
      testId={`health-glance:${key}`}
    />
  );

  const menuItems = standardPanelMenu({
    density,
    onDensityChange: setDensity,
    showTechnicalIds: showIds,
    onShowTechnicalIdsChange: setShowIds,
    getCopyData: () => ({
      team: team ?? null,
      agents: entries.map((entry) => ({ identity: entry.identity, name: entry.name, health: entry.health.kind, reason: entry.reason })),
    }),
    testIdPrefix: "health",
  });

  return (
    <PanelShell className="health" testId="health-panel" density={density} showTechnicalIds={showIds}>
      <PanelHeader
        title="Health"
        summary={summary}
        summaryTestId="health-summary"
        menu={{ label: "More actions for Health", items: menuItems, triggerTestId: "health-menu" }}
      />
      {entries.length ? (
        <GlanceStrip ariaLabel="Health at a glance" testId="health-glance">
          {figure("healthy", "Healthy", healthy, "ok", members ? `of ${plural(members, "agent")}` : undefined)}
          {figure("working", "Working", working, "live", working ? "busy; health is checked once idle" : undefined)}
          {figure("degraded", "Degraded", degraded, "warn")}
          {figure("stalled", "Stalled", stalled + broken, "crit", broken ? `${broken} broken` : undefined)}
          {figure("repair", "Needs repair", repair, "warn")}
        </GlanceStrip>
      ) : null}
      <PanelBody className="health__body">
        {entries.length === 0 ? (
          <EmptyState
            illustration="agents"
            title="No agents to report on yet"
            body="Health appears here once a team is running: each agent's status, why, and since when."
          />
        ) : (
          <>
            <PanelSection
              label={filter ? `${humanizeEnum(filter === "repair" ? "needs repair" : filter)} agents` : "Agents"}
              count={shown.length}
              headerAs="div"
              padded
              testId="health-tiles"
            >
              {tiles.length === 0 ? (
                <EmptyState inline kind="filtered" title="No agents in this state right now." />
              ) : (
                <ul className="health__tiles">
                  {tiles.map((entry) => {
                    const openable = Boolean(onOpen) && (canOpen ? canOpen(entry.identity) : true);
                    const unobservedReason = entry.health.kind === "unobserved" && entry.row.unobserved?.reason;
                    return (
                      <li key={entry.identity} className="health__cell" data-testid={`health-tile:${entry.identity}`} data-health={entry.health.kind}>
                        <EntityCard
                          kind="agent"
                          testId={`health-identity:${entry.identity}`}
                          className="health__tile"
                          dataAttributes={{ "data-health": entry.health.kind, "data-tone": entry.health.tone }}
                          mark={<AgentAvatar name={entry.name} size={22} live={entry.health.kind === "working"} />}
                          title={<span title={entry.identity}>{entry.name}</span>}
                          activateLabel={openable ? `${entry.name}, ${entry.health.word}` : undefined}
                          subtitle={tileSubtitle(entry.row) || undefined}
                          status={<StatusPill tone={entry.health.tone}>{entry.health.word}</StatusPill>}
                          now={{ text: unobservedReason ? `Not observed: ${String(unobservedReason).replace(/_/g, " ")}` : entry.reason, live: entry.health.kind === "working" }}
                          trend={{
                            values: entry.activity.hourly,
                            label: seriesWords(entry.activity.hourly, entry.activity.endMs, "event"),
                            live: entry.health.kind === "working",
                            kind: "bars",
                          }}
                          technical={showIds ? [{ label: "Identity", value: entry.identity }] : undefined}
                          live={entry.health.kind === "working"}
                          onActivate={openable ? () => onOpen?.(entry.identity) : undefined}
                        />
                      </li>
                    );
                  })}
                </ul>
              )}
              {!filter && !showAllHealthy && fine.length > fineShown.length ? (
                <button type="button" className="health__more" onClick={() => setShowAllHealthy(true)}>
                  Show all {fine.length} agents without problems
                </button>
              ) : null}
            </PanelSection>
            <PanelSection
              label="Activity by hour"
              action={<span className="health__heat-note">agents × last 24 hours</span>}
              headerAs="div"
              padded
              testId="health-activity"
            >
              <div ref={heatRef} className="health__heat-measure" aria-hidden="true" />
              <HeatStrip
                rows={heat.rows}
                columns={heatColumns}
                rowLabelWidth={HEAT_LABEL_WIDTH}
                cell={heatCell}
                gap={heatCell >= 12 ? 3 : 2}
                columnTick={(index) => (index % 3 === 0 ? heatColumns[index].slice(0, 2) : null)}
                label={heat.busiest && heat.busiest.total > 0
                  ? `Events per agent per hour over the last 24 hours; busiest ${heat.busiest.entry.name} with ${heat.busiest.total}`
                  : "No activity in the last 24 hours"}
                formatCell={(row, column, value) => `${row.label} · ${heatColumns[column]} · ${plural(value, "event")}`}
                testId="health-heat"
              />
              {heat.hidden ? <p className="health__note">and {plural(heat.hidden, "quieter agent")} not shown</p> : null}
            </PanelSection>
            <PanelSection label="Team" headerAs="div" padded testId="health-team">
              <KeyValueList
                items={[
                  { label: "Team", value: team?.running === true ? "Running" : team?.running === false ? "Stopped" : "Not observed" },
                  {
                    label: "Modules",
                    value: modules.length ? (
                      <span className="cc-chips health__modules">
                        {modules.map((name) => (
                          <Chip key={name} variant="quiet" className="health__module">{name}</Chip>
                        ))}
                      </span>
                    ) : moduleCount ? plural(moduleCount, "module") : "None loaded",
                  },
                ]}
              />
            </PanelSection>
          </>
        )}
      </PanelBody>
    </PanelShell>
  );
}
