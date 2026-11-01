// The Topology glance strip (design spec B1, L0): the agents (the hero
// figure with the brand ladder), the connections, the messages in the
// window and the problems. Each tile names the facts behind its number in
// words; the problems tile names the first one.

import React from "react";

import { BarcodeMeter, FigureTile, GlanceStrip } from "../panel-kit";
import type { Tone } from "../panel-kit/tone";
import {
  TOPOLOGY_WINDOWS,
  topologyConnectionCount,
  topologyConnectionWords,
  type TopologyCounts,
  type TopologyProblem,
  type TopologyTrafficWindow,
  type TopologyWindowId,
} from "./view-model";

export interface TopologyGlanceProps {
  counts: TopologyCounts;
  problems: readonly TopologyProblem[];
  traffic: TopologyTrafficWindow;
  window: TopologyWindowId;
  /** Messages are unknown (no traffic source at all): the tile says so. */
  trafficKnown: boolean;
  loading?: boolean;
  /** A pane under 360 px keeps only the agents and the problems. */
  narrow?: boolean;
}

function problemTone(problems: readonly TopologyProblem[]): Tone | undefined {
  if (problems.some((problem) => problem.tone === "crit")) return "crit";
  if (problems.length > 0) return "warn";
  return undefined;
}

export function TopologyGlance({
  counts,
  problems,
  traffic,
  window,
  trafficKnown,
  loading,
  narrow,
}: TopologyGlanceProps): React.JSX.Element {
  const windowWords = TOPOLOGY_WINDOWS[window];
  const connections = topologyConnectionCount(counts);
  const meterLegend = [
    `${counts.longLived} long-lived`,
    counts.helpers ? `${counts.helpers} ${counts.helpers === 1 ? "helper" : "helpers"}` : "",
    counts.working ? `${counts.working} working` : "",
  ].filter(Boolean).join(", ");
  return (
    <GlanceStrip ariaLabel="Topology at a glance" testId="topology-glance">
      <FigureTile
        hero
        label="Agents"
        value={counts.agents}
        loading={loading}
        live={counts.working > 0}
        testId="topology-glance:agents"
        meter={(
          <BarcodeMeter
            filled={counts.longLived}
            outlined={counts.helpers}
            live={counts.working}
            label={`${counts.agents} agents: ${meterLegend}`}
            legend
          />
        )}
      />
      {narrow ? null : (
        <FigureTile
          label="Connections"
          value={connections}
          loading={loading}
          testId="topology-glance:connections"
          foot={topologyConnectionWords(counts)}
        />
      )}
      {narrow ? null : (
        <FigureTile
          label={windowWords.figure}
          value={trafficKnown ? traffic.total : "-"}
          loading={loading}
          testId="topology-glance:messages"
          trend={trafficKnown && traffic.total > 0 ? traffic.bins : undefined}
          trendLabel={trafficKnown ? `${traffic.total} messages ${windowWords.phrase}` : undefined}
          delta={trafficKnown ? { value: traffic.total - traffic.previousTotal, period: windowWords.previous, goodWhen: "up" } : undefined}
          foot={trafficKnown ? undefined : "No message history yet"}
        />
      )}
      <FigureTile
        label="Problems"
        value={problems.length}
        tone={problemTone(problems)}
        loading={loading}
        testId="topology-glance:problems"
        foot={problems.length === 0
          ? "Nothing needs attention"
          : problems.length === 1
            ? problems[0]!.text
            : `${problems[0]!.text} · ${problems.length - 1} more`}
      />
    </GlanceStrip>
  );
}
