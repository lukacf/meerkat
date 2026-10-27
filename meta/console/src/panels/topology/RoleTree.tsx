// Role tree - agents grouped by role under a single root mob. Mirrors
// the production "flat variety" from the reference: one collapsible root with
// per-role sections; high-cardinality roles default-collapsed.
//
// Real activity: a section header gets a "hot" halo when any of its
// members has been active in the recent window; individual agent chips
// get a halo when they themselves have been active.

import React from "react";
import {
  buildGraph,
  colourForRole,
  roleIndexFor,
  useTopologyActivity,
} from "./data";
import type { ConsoleAgent, ConsoleFrame, ConsoleTopologyNode } from "../../types";

interface RoleTreeProps {
  nodes: ConsoleTopologyNode[];
  agents: ConsoleAgent[];
  activity: ConsoleFrame[];
}

const STATE_COLOUR: Record<string, string> = {
  active: "var(--ok)",
  running: "var(--ok)",
  idle: "var(--ink-faint)",
  degraded: "var(--warn)",
  retired: "var(--ink-faint)",
  stopped: "var(--ink-faint)",
};

function stateColour(state: string): string {
  return STATE_COLOUR[state] || "var(--ink-muted)";
}

const COLLAPSE_THRESHOLD = 12;

export function RoleTree({
  nodes,
  agents,
  activity,
}: RoleTreeProps): React.JSX.Element {
  const graph = React.useMemo(() => buildGraph(nodes, agents), [nodes, agents]);
  const roleIndex = React.useMemo(() => roleIndexFor(graph.roles), [graph.roles]);
  const live = useTopologyActivity(activity, graph, { life: 1500 });

  const grouped = React.useMemo(() => {
    const g: Record<string, typeof graph.agents> = {};
    for (const r of graph.roles) g[r] = [];
    for (const a of graph.agents) (g[a.role] ||= []).push(a);
    return g;
  }, [graph]);

  const [expanded, setExpanded] = React.useState<Record<string, boolean>>(() => {
    const initial: Record<string, boolean> = { __root: true };
    // Default: expand small role sections, collapse big ones (the production
    // pattern from the reference). Threshold is intentionally low - most
    // production deployments will exceed it for `personal:`/`channel:`
    // roles, and keeping them collapsed avoids the wall-of-chips problem.
    for (const r of graph.roles) {
      const count = grouped[r]?.length || 0;
      initial[r] = count > 0 && count <= COLLAPSE_THRESHOLD;
    }
    return initial;
  });

  const toggle = (key: string) =>
    setExpanded((s) => ({ ...s, [key]: !s[key] }));

  const rootHot = graph.agents.some((a) => live.active[a.id]);
  const rootBusy = graph.agents.some((a) => live.busy[a.id]);

  return (
    <div className="topo-roletree">
      <div className="topo-roletree__row">
        <button
          type="button"
          className={`topo-roletree__mob ${rootHot ? "is-hot" : ""}${rootBusy ? " is-busy" : ""}`}
          onClick={() => toggle("__root")}
        >
          <span
            className="topo-roletree__chevron"
            style={{ transform: expanded.__root ? "rotate(90deg)" : "rotate(0)" }}
          >
            ▸
          </span>
          <span className="topo-roletree__dot" style={{ background: "var(--ok)" }} />
          <span className="topo-roletree__label">mob</span>
          <span className="topo-roletree__count">{graph.agents.length} agents · {graph.roles.length} roles</span>
          {rootBusy && <span className="topo-roletree__busy" aria-label="agents working" />}
        </button>
      </div>
      {expanded.__root && graph.roles.map((role) => {
        const list = grouped[role] || [];
        if (list.length === 0) return null;
        const isOpen = !!expanded[role];
        const sectionHot = list.some((a) => live.active[a.id]);
        const sectionBusy = list.some((a) => live.busy[a.id]);
        const sectionBusyCount = list.filter((a) => live.busy[a.id]).length;
        const colour = colourForRole(role, roleIndex);
        return (
          <div className="topo-roletree__section" key={role}>
            <button
              type="button"
              className={`topo-roletree__role ${sectionHot ? "is-hot" : ""}${sectionBusy ? " is-busy" : ""}`}
              onClick={() => toggle(role)}
            >
              <span
                className="topo-roletree__chevron"
                style={{ transform: isOpen ? "rotate(90deg)" : "rotate(0)" }}
              >
                ▸
              </span>
              <span className="topo-roletree__dot" style={{ background: colour }} />
              <span className="topo-roletree__label">{role}</span>
              <span className="topo-roletree__count">{list.length}</span>
              {sectionBusy && (
                <span className="topo-roletree__busy" aria-label={`${sectionBusyCount} working`}>
                  <span className="topo-roletree__busy-count">{sectionBusyCount}</span>
                </span>
              )}
            </button>
            {isOpen && (
              <div className="topo-roletree__pod">
                {list.map((agent) => {
                  const isHot = !!live.active[agent.id];
                  const isBusy = !!live.busy[agent.id];
                  return (
                    <div
                      key={agent.id}
                      className={`topo-roletree__agent ${isHot ? "is-hot" : ""}${isBusy ? " is-busy" : ""}`}
                      data-testid={`topology-node:${agent.id}`}
                      title={`${agent.id}${agent.state ? " · " + agent.state : ""}${isBusy ? " · working" : ""}`}
                    >
                      <span
                        className="topo-roletree__agent-dot"
                        style={{ background: stateColour(agent.state) }}
                      />
                      <span className="topo-roletree__agent-label">{agent.label || agent.id}</span>
                      {isBusy && <span className="topo-roletree__busy" aria-label="working" />}
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
