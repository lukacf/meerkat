import React from "react";
import type { ConsoleAgent } from "../types";
import { restartOffered } from "../lib/side-chat";
import { bucketOf } from "./Sidebar";
import {
  AgentAvatar,
  AgentName,
  Button,
  Chip,
  EmptyState,
  KeyValueList,
  ListRow,
  PanelHeader,
  PanelSection,
  PanelShell,
  RelativeTime,
  RowList,
  SearchField,
  SegmentedControl,
  SplitDetail,
  StatusDot,
  TechnicalDetails,
  humanizeEnum,
  joinMeta,
  plural,
  type SegmentOption,
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
}

type Bucket = ReturnType<typeof bucketOf>;
type Filter = "all" | Bucket;
const BUCKET_ORDER: Bucket[] = ["Personal", "Coordinators", "Domains", "Internal", "Other"];

const RETIRED_STATES = new Set(["retired", "retiring", "stopped", "completed"]);

function stateWord(agent: ConsoleAgent): string {
  return (agent.state || "").trim().toLowerCase();
}

function isRetired(agent: ConsoleAgent): boolean {
  return RETIRED_STATES.has(stateWord(agent));
}

function isWorking(agent: ConsoleAgent): boolean {
  if (isRetired(agent)) return false;
  return Boolean(agent.response_phase) || agent.progress?.run_state === "run_open";
}

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
  switch (agent.response_phase) {
    case "generating":
      return "writing a reply";
    case "tool-executing":
      return "running a tool";
    case "waiting":
      return "starting a reply";
    default:
      break;
  }
  const progress = agent.progress;
  if (progress?.run_state === "run_open") {
    return progress.in_flight_work > 1 ? `working on ${progress.in_flight_work} tasks` : "working";
  }
  if (progress?.run_state === "idle") return "idle";
  return state === "active" ? "ready" : "";
}

function roleWords(agent: ConsoleAgent): string {
  return humanizeEnum(agent.role || agent.kind || "");
}

function lastChange(agent: ConsoleAgent): { words: string; at: number | null } {
  const progress = agent.progress;
  const at = progress && progress.last_progress_at_ms > 0 ? progress.last_progress_at_ms : null;
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
}: RosterPanelProps): React.JSX.Element {
  const [query, setQuery] = React.useState("");
  const [filter, setFilter] = React.useState<Filter>("all");
  const [sel, setSel] = React.useState<string>(agents[0]?.member_id || "");
  // In a narrow pane the detail replaces the list only once someone opens it.
  const [detailOpen, setDetailOpen] = React.useState(false);

  React.useEffect(() => {
    if (selectedMemberId) setSel(selectedMemberId);
  }, [selectedMemberId]);

  const buckets = React.useMemo(() => {
    const counts = new Map<Bucket, number>();
    for (const agent of agents) counts.set(bucketOf(agent), (counts.get(bucketOf(agent)) || 0) + 1);
    return BUCKET_ORDER.filter((bucket) => counts.has(bucket)).map((bucket) => ({ bucket, count: counts.get(bucket) || 0 }));
  }, [agents]);

  const rows = React.useMemo(() => {
    const needle = query.trim().toLowerCase();
    return agents.filter((agent) => {
      if (filter !== "all" && bucketOf(agent) !== filter) return false;
      if (!needle) return true;
      const hay = `${agent.label} ${agent.role || ""} ${agent.kind || ""} ${agent.member_id} ${agent.identity || ""}`.toLowerCase();
      return hay.includes(needle);
    });
  }, [agents, query, filter]);

  const active = rows.find((row) => row.member_id === sel) || rows[0];
  const activeIdentity = active?.identity || active?.member_id || "";
  const attention = agents.filter((agent) => rosterHealthChip(agent) !== null).length;
  const activeCount = agents.filter((agent) => !isRetired(agent) && stateWord(agent) !== "broken").length;

  const select = (agent: ConsoleAgent) => {
    setSel(agent.member_id);
    setDetailOpen(true);
    onSelect(agent);
  };

  const filters: SegmentOption<Filter>[] = [
    { id: "all", label: "All" },
    ...buckets.map(({ bucket }) => ({ id: bucket, label: bucket })),
  ];

  const list = rows.length === 0 ? (
    agents.length === 0 ? (
      <EmptyState title="No agents yet" body="Agents appear here once a team is running." />
    ) : (
      <EmptyState
        kind="filtered"
        title={query.trim() ? `No agents match "${query.trim()}"` : "No agents in this group"}
        action={<Button size="sm" onClick={() => { setQuery(""); setFilter("all"); }}>Clear</Button>}
      />
    )
  ) : (
    <RowList className="roster__rows">
      {rows.map((agent) => {
        const chip = rosterHealthChip(agent);
        const at = agent.progress && agent.progress.last_progress_at_ms > 0 ? agent.progress.last_progress_at_ms : null;
        return (
          <ListRow
            key={agent.member_id}
            testId={`roster-row:${agent.member_id}`}
            className="roster__row"
            dataAttributes={{ "data-state": stateWord(agent) || "unknown", "data-health": agent.progress?.health || undefined }}
            selected={Boolean(active && agent.member_id === active.member_id)}
            onActivate={() => select(agent)}
            lead={<StatusDot tone={rosterDotTone(agent)} live={isWorking(agent) && !chip} />}
            title={agent.label}
            titleAttr={agent.identity || agent.member_id}
            subtitle={joinMeta(roleWords(agent), rosterActivity(agent)) || undefined}
            meta={
              <>
                {chip ? <Chip tone={chip.tone}>{chip.label}</Chip> : null}
                {at ? <RelativeTime value={at} /> : null}
              </>
            }
          />
        );
      })}
    </RowList>
  );

  const detail = active ? (
    <RosterDetail
      agent={active}
      agents={agents}
      identity={activeIdentity}
      canResetLifecycle={canResetLifecycle}
      actionLabels={actionLabels}
      actionVisibility={actionVisibility}
      onChat={onChat}
      onDetails={onDetails}
      onLifecycle={onLifecycle}
      onSelectPeer={select}
    />
  ) : null;

  return (
    <PanelShell className="roster" testId="roster-panel">
      <PanelHeader
        title="Roster"
        subtitle={joinMeta(
          plural(agents.length, "agent"),
          `${activeCount} active`,
          attention ? `${attention} need${attention === 1 ? "s" : ""} attention` : "",
        )}
        actions={
          <>
            <SearchField label="Search agents" placeholder="Search agents" value={query} onChange={setQuery} />
            {filters.length > 2 ? (
              <SegmentedControl ariaLabel="Agent groups" value={filter} onChange={setFilter} options={filters} />
            ) : null}
          </>
        }
      />
      <SplitDetail
        className="roster__split"
        list={list}
        detail={detail}
        detailOpen={detailOpen && Boolean(active)}
        onBack={() => setDetailOpen(false)}
        backLabel="All agents"
        emptyDetail={<EmptyState inline title="Select an agent to see its details." />}
      />
    </PanelShell>
  );
}

function RosterDetail({
  agent,
  agents,
  identity,
  canResetLifecycle,
  actionLabels,
  actionVisibility,
  onChat,
  onDetails,
  onLifecycle,
  onSelectPeer,
}: {
  agent: ConsoleAgent;
  agents: ConsoleAgent[];
  identity: string;
  canResetLifecycle: boolean;
  actionLabels: RosterPanelProps["actionLabels"];
  actionVisibility: RosterPanelProps["actionVisibility"];
  onChat: (agent: ConsoleAgent) => void;
  onDetails: (agent: ConsoleAgent) => void;
  onLifecycle: RosterPanelProps["onLifecycle"];
  onSelectPeer: (agent: ConsoleAgent) => void;
}) {
  const chip = rosterHealthChip(agent);
  const activity = rosterActivity(agent);
  const change = lastChange(agent);
  const inFlight = agent.progress?.in_flight_work ?? 0;
  const peers = (agent.wired_to || []).map(peerKey).filter(Boolean);
  const showRespawn = actionVisibility?.respawn !== false && agent.affordances?.can_respawn && restartOffered(agent);
  const showReset = actionVisibility?.reset !== false && canResetLifecycle && restartOffered(agent);
  const showRetire = actionVisibility?.retire !== false && agent.affordances?.can_retire;
  return (
    <div className="roster__detail" data-testid={`roster-detail:${agent.member_id}`}>
      <div className="roster__detail-head">
        <AgentAvatar name={agent.label} size={28} />
        <div className="roster__detail-name">
          <h3 title={identity}>{agent.label}</h3>
          <div className="roster__detail-role">{joinMeta(humanizeEnum(agent.role || agent.kind || ""), agent.group ? humanizeEnum(agent.group) : "")}</div>
        </div>
        {chip ? <Chip tone={chip.tone}>{chip.label}</Chip> : null}
      </div>
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
              <Button size="sm" onClick={() => onLifecycle(identity, "meta/reset")}>{actionLabels?.reset || "Reset"}</Button>
            ) : null}
            {showRetire ? (
              <Button size="sm" variant="danger" onClick={() => onLifecycle(identity, "meta/retire")}>{actionLabels?.retire || "Retire"}</Button>
            ) : null}
          </div>
        ) : null}
      </div>
      <PanelSection label="Now" headerAs="div" padded>
        <KeyValueList
          items={[
            { label: "Status", value: activity ? activity.charAt(0).toUpperCase() + activity.slice(1) : humanizeEnum(agent.state || "unknown") },
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
          ]}
        />
      </PanelSection>
      {peers.length ? (
        <PanelSection label="Connected to" count={peers.length} headerAs="div" padded>
          <div className="cc-chips roster__peers">
            {peers.map((peer) => {
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
        </PanelSection>
      ) : null}
      <div className="roster__tech">
        <TechnicalDetails
          items={[
            { label: "Identity", value: agent.identity },
            agent.member_id !== agent.identity ? { label: "Member", value: agent.member_id } : null,
            { label: "Session", value: agent.session_id },
            typeof agent.generation === "number" ? { label: "Generation", value: agent.generation } : null,
            { label: "Kind", value: agent.kind, plain: true },
            { label: "Profile", value: agent.role, plain: true },
          ]}
        />
      </div>
    </div>
  );
}
