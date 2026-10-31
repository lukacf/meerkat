// The Graph section of the dev-only kit gallery (?kit_gallery=1, K1): lanes,
// layered frames, 300 agents in clusters, the ego graph, force, the list
// twin, particles in flight, and the first-render measurements. The gallery
// discovers this file by its *.gallery.tsx name; nothing in the product
// imports it.

import React from "react";

import { GraphView } from "./graph-view";
import { GraphList } from "./list-twin";
import { StatusSquare } from "./status-square";
import { demoEgo, demoPlan, demoPopulation, demoTeam, messagePairs, seeded } from "./gallery-data";
import type { DemoGraph } from "./gallery-data";
import type { VizMemoryEvent, VizMessage } from "./types";
import { graphSummary } from "./words";

/**
 * Until K1's tokens land in a branch, the gallery defines the A2.3
 * visualisation tokens and the A2.2 accent on its own root, from the spec's
 * formulas. Where tokens.css defines them it does nothing.
 */
const SPEC_TOKENS: Record<string, string> = {
  "--_cc-viz-ink": "var(--_cc-foreground)",
  "--_cc-viz-mid": "color-mix(in srgb, var(--_cc-foreground) 55%, var(--_cc-surface-card))",
  "--_cc-viz-faint": "color-mix(in srgb, var(--_cc-foreground) 22%, var(--_cc-surface-card))",
  "--_cc-viz-edge": "color-mix(in srgb, var(--_cc-foreground) 30%, var(--_cc-surface-card))",
  "--_cc-viz-edge-strong": "color-mix(in srgb, var(--_cc-foreground) 62%, var(--_cc-surface-card))",
  "--_cc-viz-grid": "color-mix(in srgb, var(--_cc-foreground) 8%, var(--_cc-surface-card))",
  "--_cc-viz-plate": "var(--_cc-surface-card)",
  "--_cc-viz-live": "var(--_cc-member-accent)",
  "--_cc-viz-live-soft": "color-mix(in srgb, var(--_cc-member-accent) 18%, var(--_cc-surface-card))",
};
const SPEC_ACCENT: Record<"light" | "dark", Record<string, string>> = {
  light: { "--_cc-member-accent": "oklch(0.56 0.165 40)", "--_cc-member-accent-soft": "oklch(0.93 0.045 40)" },
  dark: { "--_cc-member-accent": "oklch(0.72 0.16 40)", "--_cc-member-accent-soft": "oklch(0.32 0.08 40)" },
};

function useSpecTokenShim(ref: React.RefObject<HTMLElement | null>): void {
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element || typeof getComputedStyle !== "function") return;
    if (getComputedStyle(element).getPropertyValue("--_cc-viz-ink").trim()) return;
    const theme = element.closest("[data-cc-theme]")?.getAttribute("data-cc-theme") === "dark" ? "dark" : "light";
    const variant = element.closest("[data-cc-variant]")?.getAttribute("data-cc-variant") ?? "rams";
    const values = { ...(variant === "rams" ? SPEC_ACCENT[theme] : {}), ...SPEC_TOKENS };
    for (const [name, value] of Object.entries(values)) element.style.setProperty(name, value);
  }, [ref]);
}

function useMessageStream(graph: DemoGraph, every: number, seed: number): VizMessage[] {
  const pairs = React.useMemo(() => messagePairs(graph), [graph]);
  const [messages, setMessages] = React.useState<VizMessage[]>([]);
  React.useEffect(() => {
    if (pairs.length === 0) return undefined;
    const random = seeded(seed);
    let count = 0;
    const timer = setInterval(() => {
      const [from, to] = pairs[Math.floor(random() * pairs.length)];
      count += 1;
      const message = { id: `m${seed}-${count}`, from, to, at: Date.now() };
      setMessages((list) => [...list.slice(-24), message]);
    }, every);
    return () => clearInterval(timer);
  }, [pairs, every, seed]);
  return messages;
}

function useMemoryStream(): VizMemoryEvent[] {
  const [events, setEvents] = React.useState<VizMemoryEvent[]>([]);
  React.useEffect(() => {
    const plan: [string, VizMemoryEvent["op"]][] = [["support", "write"], ["triage", "read"], ["billing", "read"], ["router", "quarantine"]];
    let count = 0;
    const timer = setInterval(() => {
      const [node, op] = plan[count % plan.length];
      count += 1;
      setEvents((list) => [...list.slice(-8), { id: `mem${count}`, node, op, at: Date.now() }]);
    }, 1300);
    return () => clearInterval(timer);
  }, []);
  return events;
}

function Demo({ title, note, height, children, testId }: { title: string; note: string; height: number; children: React.ReactNode; testId: string }): React.JSX.Element {
  return (
    <section className="cc-viz-gallery__demo" data-testid={testId}>
      <header className="cc-viz-gallery__head">
        <h3>{title}</h3>
        <p>{note}</p>
      </header>
      <div className="cc-viz-gallery__frame" style={{ height }}>
        {children}
      </div>
    </section>
  );
}

function LanesDemo(): React.JSX.Element {
  const team = React.useMemo(() => demoTeam(), []);
  const messages = useMessageStream(team, 520, 11);
  const memoryEvents = useMemoryStream();
  const [selection, setSelection] = React.useState<string[]>(["triage"]);
  return (
    <GraphView
      nodes={team.nodes}
      edges={team.edges}
      lanes={team.lanes}
      layout="lanes"
      ariaLabel={graphSummary(team.nodes, team.edges)}
      selection={selection}
      onSelect={setSelection}
      messages={messages}
      memoryEvents={memoryEvents}
      testIdPrefix="gallery-lanes"
      cacheKey="gallery:lanes"
    />
  );
}

function PlanDemo(): React.JSX.Element {
  const plan = React.useMemo(() => demoPlan(), []);
  const [selection, setSelection] = React.useState<string[]>(["publish"]);
  const accentPath = React.useMemo(() => ({ nodes: plan.criticalNodes, edges: plan.criticalEdges }), [plan]);
  return (
    <GraphView
      nodes={plan.nodes}
      edges={plan.edges}
      frames={plan.frames}
      layout="layered"
      ariaLabel={graphSummary(plan.nodes, plan.edges)}
      selection={selection}
      onSelect={setSelection}
      accentPath={accentPath}
      hoverHighlight="chain"
      edgeLabels="always"
      legend="collapsed"
      testIdPrefix="gallery-plan"
    />
  );
}

function ClustersDemo(): React.JSX.Element {
  const population = React.useMemo(() => demoPopulation(300, 8), []);
  const messages = useMessageStream(population, 380, 5);
  return (
    <GraphView
      nodes={population.nodes}
      edges={population.edges}
      lanes={population.lanes}
      layout="clusters"
      ariaLabel={graphSummary(population.nodes, population.edges)}
      messages={messages}
      testIdPrefix="gallery-clusters"
    />
  );
}

function EgoDemo(): React.JSX.Element {
  const ego = React.useMemo(() => demoEgo(), []);
  return (
    <GraphView
      nodes={ego.nodes}
      edges={ego.edges}
      layout="radial"
      center="billing"
      ariaLabel={`Billing and its ${ego.nodes.length - 1} connections`}
      legend="off"
      minimap="off"
      testIdPrefix="gallery-ego"
    />
  );
}

function ForceDemo(): React.JSX.Element {
  const population = React.useMemo(() => demoPopulation(48, 4, 19), []);
  return (
    <GraphView
      nodes={population.nodes}
      edges={population.edges}
      lanes={population.lanes}
      layout="force"
      ariaLabel={graphSummary(population.nodes, population.edges)}
      testIdPrefix="gallery-force"
      cacheKey="gallery:force"
    />
  );
}

function ListDemo(): React.JSX.Element {
  const team = React.useMemo(() => demoTeam(), []);
  const [selection, setSelection] = React.useState<string[]>(["triage"]);
  return (
    <GraphList
      nodes={team.nodes}
      edges={team.edges}
      lanes={team.lanes}
      selection={selection}
      onSelect={setSelection}
      onOpen={() => undefined}
      ariaLabel="Agents and their connections"
      testIdPrefix="gallery-list"
    />
  );
}

interface Measurement {
  label: string;
  ms: number;
  budget: number;
}

/** Mounts a 100-node SVG graph, then a 1000-node canvas graph, and reports each first paint. */
function RenderTimes(): React.JSX.Element {
  const runs = React.useMemo(
    () => [
      { label: "100 nodes, lanes, SVG", graph: demoPopulation(100, 6, 23), layout: "lanes" as const, renderer: "svg" as const, budget: 100 },
      { label: "1000 nodes, clusters, canvas", graph: demoPopulation(1000, 8, 29), layout: "clusters" as const, renderer: "canvas" as const, budget: 400 },
    ],
    [],
  );
  const [stage, setStage] = React.useState(-1);
  const [results, setResults] = React.useState<Measurement[]>([]);
  const start = () => {
    setResults([]);
    setStage(0);
  };
  const current = stage >= 0 && stage < runs.length ? runs[stage] : null;
  return (
    <div className="cc-viz-gallery__perf">
      <div className="cc-viz-gallery__perf-bar">
        <button type="button" data-testid="graph-gallery-perf-run" onClick={start}>
          Measure first render
        </button>
        <output data-testid="graph-gallery-perf" data-done={stage >= runs.length ? "true" : "false"}>
          {results.map((result) => `${result.label}: ${result.ms.toFixed(1)} ms (budget ${result.budget} ms)`).join(" · ") || "Not measured yet"}
        </output>
      </div>
      <div className="cc-viz-gallery__perf-stage">
        {current ? (
          <GraphView
            key={stage}
            nodes={current.graph.nodes}
            edges={current.graph.edges}
            lanes={current.graph.lanes}
            layout={current.layout}
            renderer={current.renderer}
            ariaLabel={graphSummary(current.graph.nodes, current.graph.edges)}
            onFirstRender={(ms) => {
              setResults((list) => [...list, { label: current.label, ms, budget: current.budget }]);
              setTimeout(() => setStage((value) => value + 1), 300);
            }}
          />
        ) : (
          <div className="cc-viz-gallery__perf-empty">
            <StatusSquare status={stage >= runs.length ? "completed" : "open"} />
            {stage >= runs.length ? "Measured" : "Press Measure first render"}
          </div>
        )}
      </div>
    </div>
  );
}

export function GraphGallery(): React.JSX.Element {
  const ref = React.useRef<HTMLDivElement | null>(null);
  useSpecTokenShim(ref);
  return (
    <div className="cc-viz-gallery" ref={ref}>
      <Demo testId="graph-gallery:lanes" title="Lanes" note="Topology: every relation of the A3.9 grammar, a live coordinator, particles in flight, memory arcs, a degraded and a wedged agent, helpers under their hosts." height={600}>
        <LanesDemo />
      </Demo>
      <Demo testId="graph-gallery:layered" title="Layered with frames" note="WorkGraph: goals as frames with progress, a nested goal, the critical path in the accent, a selected blocked item, a collapsed done goal." height={620}>
        <PlanDemo />
      </Demo>
      <Demo testId="graph-gallery:clusters" title="Clusters, 300 agents" note="Eight groups as rings of member dots around their leads; helpers hollow, live in the accent; one bundled line per group pair." height={640}>
        <ClustersDemo />
      </Demo>
      <Demo testId="graph-gallery:radial" title="Radial ego graph" note="Billing at the centre, its peers grouped by relation (Agent details, 240 px)." height={240}>
        <EgoDemo />
      </Demo>
      <Demo testId="graph-gallery:force" title="Force, 48 agents" note="d3-force in idle chunks, pulled to group anchors; positions cached for the session." height={480}>
        <ForceDemo />
      </Demo>
      <Demo testId="graph-gallery:list" title="List twin" note="The same team as rows: the screen reader path and the List view." height={420}>
        <ListDemo />
      </Demo>
      <Demo testId="graph-gallery:perf" title="First render" note="Budgets (A4.9): 100 nodes under 100 ms; 1000 nodes under 400 ms on canvas." height={420}>
        <RenderTimes />
      </Demo>
    </div>
  );
}

export const gallerySections = [
  {
    id: "graph",
    title: "Graph",
    order: 90,
    description: "GraphView (A3.9): lanes, layered frames, clusters, radial, force, the list twin and particles.",
    render: () => <GraphGallery />,
  },
];
