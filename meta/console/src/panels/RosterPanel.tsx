import React from "react";
import type { ConsoleAgent } from "../types";

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

const ROLE_BUCKETS = ["all", "personal", "coordinator", "domain", "internal"] as const;
type Role = typeof ROLE_BUCKETS[number];

function roleOf(a: ConsoleAgent): Role {
  const p = (a.role || a.kind || "").toLowerCase();
  const g = (a.group || "").toLowerCase();
  if (p.includes("personal") || g.includes("personal")) return "personal";
  if (p.includes("coord") || p.includes("triage") || p.includes("router")) return "coordinator";
  if (p.includes("monitor") || p.includes("scribe") || p.includes("gate")) return "internal";
  return "domain";
}

function stateLabel(state?: string): string {
  return (state || "unknown").toLowerCase();
}

function displayPeer(peer: unknown): string {
  if (typeof peer === "string") return peer.split("/").pop() || peer;
  if (peer && typeof peer === "object") {
    const record = peer as Record<string, unknown>;
    const value = record.label ?? record.display_name ?? record.name ?? record.identity ?? record.member_id ?? record.id;
    if (typeof value === "string") return value.split("/").pop() || value;
  }
  return "";
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
  const [q, setQ] = React.useState("");
  const [role, setRole] = React.useState<Role>("all");
  const [sel, setSel] = React.useState<string>(agents[0]?.member_id || "");

  React.useEffect(() => {
    if (selectedMemberId) setSel(selectedMemberId);
  }, [selectedMemberId]);

  const rows = React.useMemo(() => {
    return agents.filter((a) => {
      if (role !== "all" && roleOf(a) !== role) return false;
      if (!q) return true;
      const hay = `${a.label} ${a.member_id} ${a.identity || ""} ${a.role || ""} ${a.kind || ""}`.toLowerCase();
      return hay.includes(q.toLowerCase());
    });
  }, [agents, q, role]);

  const active = rows.find((r) => r.member_id === sel) || rows[0];
  const activeIdentity = active?.identity || active?.member_id || "";
  const activePeers = (active?.wired_to || []).map(displayPeer).filter(Boolean);

  return (
    <div className="view roster" data-testid="roster-panel">
      <div className="view__head">
        <h2>Roster</h2>
        <span className="view__sub">{rows.length} of {agents.length} agents</span>
        <span className="view__spacer" />
        <input
          className="view__search"
          placeholder="Filter agents, profiles, ids…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <div className="view__segs">
          {ROLE_BUCKETS.map((r) => (
            <button key={r} className={role === r ? "is-active" : ""} onClick={() => setRole(r)}>{r}</button>
          ))}
        </div>
      </div>
      <div className="roster__body">
        <div className="roster__table">
          <div className="roster__row roster__row--head">
            <span>Name</span><span>Role</span><span>State</span><span>Profile</span><span>Gen</span>
          </div>
          {rows.map((r) => {
            const isSel = active && r.member_id === active.member_id;
            return (
              <div
                key={r.member_id}
                className={`roster__row ${isSel ? "is-selected" : ""}`}
                data-state={stateLabel(r.state)}
                data-health={r.progress?.health || undefined}
                onClick={() => { setSel(r.member_id); onSelect(r); }}
                data-testid={`roster-row:${r.member_id}`}
              >
                <span className="roster__name">
                  <span className="roster__dot" />
                  <span>
                    <div>{r.label}</div>
                    <div className="roster__id">{r.identity || r.member_id}</div>
                  </span>
                </span>
                <span>{roleOf(r)}</span>
                <span className="roster__state">{stateLabel(r.state)}</span>
                <span className="mono dim">{r.role || "—"}</span>
                <span className="mono">{r.generation ?? "—"}</span>
              </div>
            );
          })}
        </div>
        <aside className="roster__detail">
          {active && (
            <>
              <div className="rd__head">
                <div className="rd__title">{active.label}</div>
                <div className="rd__id">{active.identity || active.member_id}</div>
                <div className="rd__tags">
                  {[active.role, active.kind, roleOf(active)].filter(Boolean).map((t) => (
                    <span key={String(t)} className="chip">{String(t)}</span>
                  ))}
                </div>
              </div>
              <dl className="rd__grid">
                <dt>Profile</dt><dd>{active.role || "—"}</dd>
                <dt>Kind</dt><dd>{active.kind || "—"}</dd>
                <dt>Role</dt><dd>{roleOf(active)}</dd>
                <dt>State</dt><dd><span className="roster__state">{stateLabel(active.state)}</span></dd>
                <dt>Member</dt><dd className="mono">{active.member_id}</dd>
                <dt>Identity</dt><dd className="mono">{active.identity || "—"}</dd>
                <dt>Session</dt><dd className="mono">{active.session_id || "—"}</dd>
                <dt>Generation</dt><dd className="mono">{active.generation ?? "—"}</dd>
                {active.progress && (
                  <>
                    <dt>Health</dt>
                    <dd>
                      <span
                        className="roster__state"
                        data-health={active.progress.health}
                      >
                        {active.progress.health}
                      </span>
                    </dd>
                    <dt>Run state</dt><dd className="mono">{active.progress.run_state}</dd>
                    <dt>In flight</dt><dd className="mono">{active.progress.in_flight_work}</dd>
                    <dt>Last progress</dt>
                    <dd className="mono dim">
                      {active.progress.last_progress_at_ms > 0
                        ? `${new Date(active.progress.last_progress_at_ms).toLocaleTimeString()} (${active.progress.last_progress_event})`
                        : active.progress.last_progress_event}
                    </dd>
                  </>
                )}
                <dt>Wired</dt>
                <dd>
                  {activePeers.length > 0 ? (
                    <span className="rd__peers">
                      {activePeers.map((peer) => (
                        <span className="chip" key={peer}>{peer}</span>
                      ))}
                    </span>
                  ) : (
                    <span className="mono dim">none</span>
                  )}
                </dd>
              </dl>
              <div className="rd__actions">
                {actionVisibility?.inspect !== false ? <button onClick={() => onDetails(active)}>{actionLabels?.inspect || "Details"}</button> : null}
                {actionVisibility?.chat !== false ? <button onClick={() => onChat(active)}>{actionLabels?.chat || "Open chat"}</button> : null}
                {actionVisibility?.respawn !== false && active.affordances?.can_respawn ? (
                  <button onClick={() => onLifecycle(activeIdentity, "meta/respawn")}>{actionLabels?.respawn || "Respawn"}</button>
                ) : null}
                {actionVisibility?.reset !== false && canResetLifecycle ? (
                  <button onClick={() => onLifecycle(activeIdentity, "meta/reset")}>{actionLabels?.reset || "Reset"}</button>
                ) : null}
                {actionVisibility?.retire !== false && active.affordances?.can_retire ? (
                  <button className="danger" onClick={() => onLifecycle(activeIdentity, "meta/retire")}>{actionLabels?.retire || "Retire"}</button>
                ) : null}
              </div>
            </>
          )}
        </aside>
      </div>
    </div>
  );
}
