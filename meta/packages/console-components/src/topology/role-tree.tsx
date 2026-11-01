// Role tree: the team's agents grouped by role under one root (the Roles
// view). Each role is a collapsible section headed in the lanes' micro-label
// words; an agent is a chip with its status mark (shape, tone and, in the
// tooltip, the word). It draws no edges. Large roles start collapsed.

import React from "react";

import { StatusMark } from "../panel-kit";
import { humanizeEnum, plural } from "../panel-kit/format";
import type { TopoAgent, TopoGraph } from "./data";
import type { ConsoleTopologyAgentStatus } from "./types";
import { topologyStatusPresentation } from "./view-model";

interface RoleTreeProps {
  graph: TopoGraph;
  statusById: ReadonlyMap<string, ConsoleTopologyAgentStatus>;
  /** When given, agent chips are buttons that select the agent. */
  onSelectAgent?: (agentId: string) => void;
  /** The root's name: the team's display name when the host has one. */
  teamName?: string;
  /** The team's key (its id), the root's tooltip, as a role's key is its heading's. */
  teamKey?: string;
}

const COLLAPSE_THRESHOLD = 12;

function Chevron({ open }: { open: boolean }): React.JSX.Element {
  return (
    <span className={`topo-roletree__chevron${open ? " is-open" : ""}`} aria-hidden="true">
      <svg viewBox="0 0 12 12" width="10" height="10"><path d="M4 2.5 7.5 6 4 9.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
    </span>
  );
}

export function RoleTree({
  graph,
  statusById,
  onSelectAgent,
  teamName,
  teamKey,
}: RoleTreeProps): React.JSX.Element {
  const agents = React.useMemo(
    () => graph.agents.filter((agent) => agent.kind !== "external" && !statusById.get(agent.id)?.retired),
    [graph.agents, statusById],
  );
  const grouped = React.useMemo(() => {
    const byRole = new Map<string, TopoAgent[]>();
    for (const agent of agents) {
      const list = byRole.get(agent.role);
      if (list) list.push(agent);
      else byRole.set(agent.role, [agent]);
    }
    for (const list of byRole.values()) list.sort((left, right) => left.label.localeCompare(right.label));
    return Array.from(byRole).sort(([left], [right]) => left.localeCompare(right));
  }, [agents]);

  const [expanded, setExpanded] = React.useState<Record<string, boolean>>(() => {
    const initial: Record<string, boolean> = { __root: true };
    // Small roles open, big ones collapsed: the wall-of-chips problem.
    for (const [role, list] of grouped) initial[role] = list.length > 0 && list.length <= COLLAPSE_THRESHOLD;
    return initial;
  });
  const toggle = (key: string) => setExpanded((state) => ({ ...state, [key]: !(state[key] ?? true) }));
  const isOpen = (key: string) => expanded[key] ?? true;
  const working = (list: readonly TopoAgent[]) => list.filter((agent) => statusById.get(agent.id)?.working).length;
  const rootWorking = working(agents);

  return (
    <div className="topo-roletree" data-testid="topology-roles">
      <div className="topo-roletree__row">
        <button
          type="button"
          className={`topo-roletree__mob${rootWorking ? " is-busy" : ""}`}
          aria-expanded={isOpen("__root")}
          onClick={() => toggle("__root")}
        >
          <Chevron open={isOpen("__root")} />
          <span className="topo-roletree__label" title={teamKey?.trim() || undefined}>{teamName?.trim() || "Team"}</span>
          <span className="topo-roletree__count">
            {plural(agents.length, "agent")} · {plural(grouped.length, "role")}
            {rootWorking ? ` · ${rootWorking} working now` : ""}
          </span>
        </button>
      </div>
      {isOpen("__root") && grouped.map(([role, list]) => {
        const open = isOpen(role);
        const busy = working(list);
        return (
          <div className="topo-roletree__section" key={role}>
            <button
              type="button"
              className={`topo-roletree__role${busy ? " is-busy" : ""}`}
              aria-expanded={open}
              onClick={() => toggle(role)}
            >
              <Chevron open={open} />
              <span className="topo-roletree__label" title={role}>{humanizeEnum(role) || role}</span>
              <span className="topo-roletree__count">{list.length}</span>
              {busy ? (
                <span className="topo-roletree__busy">
                  <StatusMark tone="live" />
                  <span>{busy} working</span>
                </span>
              ) : null}
            </button>
            {open && (
              <div className="topo-roletree__pod">
                {list.map((agent) => {
                  const status = statusById.get(agent.id) || { working: false };
                  const presentation = topologyStatusPresentation(status);
                  const chip = (
                    <>
                      <StatusMark tone={presentation.tone} />
                      <span className="topo-roletree__agent-label">{agent.label || agent.id}</span>
                    </>
                  );
                  const chipClass = `topo-roletree__agent${status.working ? " is-busy" : ""}${agent.kind === "helper" ? " is-helper" : ""}`;
                  const chipTitle = `${agent.id} · ${presentation.word}`;
                  return onSelectAgent ? (
                    <button
                      key={agent.id}
                      type="button"
                      className={`${chipClass} is-interactive`}
                      data-testid={`topology-node:${agent.id}`}
                      title={chipTitle}
                      onClick={() => onSelectAgent(agent.id)}
                    >
                      {chip}
                    </button>
                  ) : (
                    <div
                      key={agent.id}
                      className={chipClass}
                      data-testid={`topology-node:${agent.id}`}
                      title={chipTitle}
                    >
                      {chip}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
