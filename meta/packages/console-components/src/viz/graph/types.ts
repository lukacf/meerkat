// The graph kit's model (design spec A3.9): one GraphView draws Topology,
// WorkGraph, the agent ego graph and every other relation picture. Panels map
// their data onto these plain records; the view owns layout, drawing,
// navigation, live motion and accessibility.

import type { CSSProperties, ReactNode } from "react";

import type { Tone } from "../../panel-kit/tone";

/** What a node is. Picks its mark (the A3.9 node grammar). */
export type VizNodeKind =
  /** A long-lived member: a filled ink circle. */
  | "agent"
  /** A coordinator or lead: a filled circle with a ring. */
  | "coordinator"
  /** A spawned helper, fork or side chat: a hollow circle. */
  | "helper"
  /** An operator or console user: a ring with a centre dot. */
  | "person"
  /** A channel, route source, schedule or webhook: a hollow square. */
  | "source"
  /** A memory store or knowledge base: a cylinder. */
  | "store"
  /** An agent in another team or runtime: a mid circle in a dashed ring. */
  | "external"
  /** An unobserved member: a dashed hollow circle. */
  | "unknown"
  /** A work item: a card with its status square. */
  | "item"
  /** A goal: the header of the frame with the same id (layered layout). */
  | "goal";

/** What an edge says about its two ends. Picks its stroke (the A3.9 edge grammar). */
export type VizRelation =
  /** Comms trust, both ways: solid hairline. */
  | "wired"
  /** Messages in the window: strong hairline, width by count, chevron when one way. */
  | "traffic"
  /** A route from a source to its recipient: dashed, chevron at the recipient. */
  | "route"
  /** Parent to helper, fork or side chat: dotted, small chevron at the child. */
  | "lineage"
  /** Another team or runtime: a double hairline. */
  | "cross"
  /** A work item blocks another: strong solid, chevron at the blocked item. */
  | "blocks"
  /** A work item outside its parent's frame: dotted. */
  | "parent"
  /** Related, replaces, derived from: dotted, labelled. */
  | "related";

/**
 * An edge's state over its relation. `pending`: a connect or disconnect in
 * flight. `failed`: deliveries failed on it. `declared`: declared or desired
 * but not connected (a degraded wiring).
 */
export type VizEdgeState = "normal" | "pending" | "failed" | "declared";

/** A node's tone: the kit's status tones plus `live` (happening now). */
export type VizTone = Tone | "live";

/** The work item states the status square draws. Any other word draws as open. */
export type VizItemStatus = "open" | "ready" | "in_progress" | "blocked" | "completed" | "failed" | "cancelled";

/**
 * The node's own memory, drawn as an arc on its left. `none`: no arc. `idle`:
 * the quiet arc. `write`, `read` and `quarantine` draw it lit (the panel holds
 * them; for a timed 900 ms flash pass `memoryEvents` instead).
 */
export type VizMemoryState = "none" | "idle" | "write" | "read" | "quarantine";

/**
 * How nodes are placed. `lanes`: horizontal bands (Topology). `layered`: a DAG
 * in frames, left to right (WorkGraph, Routing columns, memory versions).
 * `force`: groups pulled together (switches to `clusters` above 150 nodes).
 * `clusters`: each group as concentric rings of member dots. `radial`: one
 * centre node with its peers on a ring, grouped by relation (ego graph).
 */
export type VizLayoutKind = "lanes" | "layered" | "force" | "clusters" | "radial";

/** `data-*` attributes rendered on an element as given (tests pin them). */
export type VizAttributes = Readonly<Record<`data-${string}`, string | number | boolean | undefined>>;

export interface VizNode {
  /** Stable id; positions, selection and test ids key on it. */
  id: string;
  kind: VizNodeKind;
  /** The name people read. */
  label: string;
  /** Second line: role ("Lead · Domain") or an item's owner, priority and age. */
  sublabel?: string;
  /** Lanes: the band. Force and clusters: the group. Radial: the relation group. */
  lane?: string;
  /** Layered: the frame (goal) the node sits in. A goal node's own frame is its id. */
  frame?: string;
  /** Order within its lane, rank or ring (the manual order); else by label. */
  order?: number;
  /** Layered: pins the node to this column (fixed columns). */
  rank?: number;
  /** Lanes: a helper hangs off this node, right of it and 36 px down. */
  anchor?: string;
  /** Status tone; `crit` and `warn` also draw the health badge unless `badge` says otherwise. */
  tone?: VizTone;
  /** The raw status word ("idle", "in_progress"); items draw it as their status square. */
  status?: string;
  /** The status in words when the raw word does not read well ("In progress"). */
  statusLabel?: string;
  /** Happening now (a turn or tool running): live fill and the pulse ring. */
  live?: boolean;
  /** Drawn at reduced emphasis (context the panel wants quiet). */
  dim?: boolean;
  /** Retired: hollow at 40 %, hidden unless the view shows retired nodes. */
  retired?: boolean;
  /** Health badge at the upper right: a warn triangle or a crit diamond. */
  badge?: "warn" | "crit" | null;
  memory?: VizMemoryState;
  size?: "sm" | "md" | "lg";
  /** Always labelled, like the top 15 % by degree. */
  hub?: boolean;
  /** Item cards: an owner initial drawn before the sublabel. */
  avatar?: string;
  /** Goal nodes: a progress ring between the title and the meta (done ink, in progress accent, blocked warning). */
  progress?: VizProgress;
  /** Goal nodes: a right-aligned trailer ("Coordinator · Leading the work") with an optional avatar initial. */
  trailing?: { text: string; avatar?: string };
  /** Extra classes on the node element (classes tests pin). */
  className?: string;
  attributes?: VizAttributes;
  /** Replaces the derived accessible name ("Billing, Lead · Domain, idle, 3 connections"). */
  ariaLabel?: string;
  /** Hover tooltip text; lines split on "\n". Defaults to label, sublabel and status. */
  title?: string;
  /** Payload for the panel's drawer; the view never reads it. */
  data?: unknown;
}

/** A goal's progress in items. */
export interface VizProgress {
  done: number;
  inProgress?: number;
  blocked?: number;
  total: number;
}

export interface VizEdge {
  /** Stable id; selection and particles key on it. */
  id: string;
  from: string;
  to: string;
  relation: VizRelation;
  /** Draw a chevron at `to`. Defaults: route, lineage and blocks yes; others no. */
  directed?: boolean;
  /** Traffic: messages in the window (1-2 thin, 3-9 medium, 10+ thick). */
  weight?: number;
  /** A word or count on a plate ("12/h", "replaces"). */
  label?: string;
  state?: VizEdgeState;
  /**
   * The mark at the midpoint: a warn triangle or a crit diamond. Defaults to
   * warn for `declared` and crit for `failed`; null draws none (an
   * operator-suppressed declared pair is intentional, not degraded).
   */
  mark?: "warn" | "crit" | null;
  /** Hover tooltip text ("Triage to Support · 12 messages in the last hour"). */
  title?: string;
  className?: string;
  attributes?: VizAttributes;
  data?: unknown;
}

export interface VizLane {
  id: string;
  /** Drawn as the band's micro-label ("COORDINATORS"; the view upper-cases it). */
  label: string;
}

export interface VizFrame {
  id: string;
  label: string;
  /** Beside the label ("3 of 5 done"). */
  meta?: string;
  /** Nested frames (depth 2 at most) sit inside their parent. */
  parent?: string;
  /** Stacking order, top to bottom; else as given. */
  order?: number;
  /** Header only: its items are not drawn (a done goal). */
  collapsed?: boolean;
  className?: string;
  attributes?: VizAttributes;
}

/** A message to play as a particle along `from` to `to`. Played once per id. */
export interface VizMessage {
  /** Defaults to `${from}>${to}@${at}`. */
  id?: string;
  from: string;
  to: string;
  /** Epoch ms when it was sent; older than 10 s at first sight is not played. */
  at: number;
}

/** A memory use to flash on the node's arc for 900 ms. Played once per id. */
export interface VizMemoryEvent {
  id?: string;
  node: string;
  op: "write" | "read" | "quarantine";
  at: number;
}

/** The pan and zoom: screen = world * scale + (tx, ty). */
export interface VizViewport {
  tx: number;
  ty: number;
  scale: number;
}

/** A placed node in layout coordinates (px at zoom 1); x and y are the centre. */
export interface VizNodeBox {
  id: string;
  x: number;
  y: number;
  /** Box width and height (cards) or the mark's diameter (marks). */
  w: number;
  h: number;
}

/** Test ids. A string names every element of that part alike; a function names each. */
export interface GraphTestIds {
  root?: string;
  svg?: string;
  viewport?: string;
  node?: string | ((node: VizNode) => string);
  edge?: string | ((edge: VizEdge) => string);
  edgeLabel?: string | ((edge: VizEdge) => string);
  frame?: string | ((frame: VizFrame) => string);
  lane?: string | ((lane: VizLane) => string);
  legend?: string;
  legendItem?: (key: string) => string;
  minimap?: string;
  zoom?: (action: VizZoomAction) => string;
}

export type VizZoomAction = "in" | "out" | "reset" | "fit" | "minimap";

/** Classes added next to the view's own `cc-viz-*` classes (classes tests pin). */
export interface GraphClassNames {
  node?: string;
  itemTitle?: string;
  itemMeta?: string;
  edge?: string;
  edgeLabel?: string;
  frame?: string;
  legendHeading?: string;
}

export interface GraphViewProps {
  nodes: readonly VizNode[];
  edges: readonly VizEdge[];
  layout: VizLayoutKind;
  /** Lanes, top to bottom. Nodes in no listed lane go to a last, unlabelled band. */
  lanes?: readonly VizLane[];
  frames?: readonly VizFrame[];
  /** Radial: the node at the centre. */
  center?: string;
  density?: "comfortable" | "compact";

  /** Selected node ids (controlled). Omit to let the view keep its own. */
  selection?: readonly string[];
  onSelect?: (ids: string[]) => void;
  /** Double-click or Enter on a node. */
  onOpen?: (id: string) => void;
  selectedEdge?: string | null;
  onEdgeSelect?: (id: string | null) => void;
  /** Brings this node into view and makes it the graph's tab stop. */
  focusId?: string | null;
  onFocusChange?: (id: string | null) => void;
  onHover?: (target: { node?: string; edge?: string } | null) => void;
  /** What stays at full ink while a node is hovered: its neighbours, or its whole upstream and downstream chain. */
  hoverHighlight?: "neighbours" | "chain";
  /** Drawn in the accent (the WorkGraph critical path): edges 2 px, nodes with an accent rule. */
  accentPath?: { nodes: readonly string[]; edges: readonly string[] } | null;

  messages?: readonly VizMessage[];
  memoryEvents?: readonly VizMemoryEvent[];

  /**
   * `auto`: shown when two or more relations or node kinds are drawn.
   * `collapsed`: shown, starting folded to its "Legend" chip.
   */
  legend?: "auto" | "on" | "off" | "collapsed";
  /**
   * One meta line at the foot of the legend ("Routes need routing access").
   * Shown whenever the legend is not "off", even where "auto" would hide it.
   */
  legendNote?: ReactNode;
  /** `auto`: shown when the content exceeds the view by 20 % or zoom > 1. */
  minimap?: "auto" | "on" | "off";
  /** The zoom controls (bottom left). */
  controls?: boolean;
  /** `auto`: SVG up to 300 nodes and 800 edges, canvas above. */
  renderer?: "auto" | "svg" | "canvas";
  /** `auto`: the A3.9 level of detail. */
  labels?: "auto" | "all" | "hover";
  /** `auto`: on hover, selection, or zoom >= 1.6 on edges longer than 120 px. */
  edgeLabels?: "auto" | "always" | "hover";
  showRetired?: boolean;
  /** Relations the legend has turned off (controlled). Omit to let the view keep its own. */
  hiddenRelations?: readonly VizRelation[];
  onHiddenRelationsChange?: (relations: VizRelation[]) => void;
  /** "<panel>:<scope>": positions persist in sessionStorage under meta-console-viz:<panel>:<scope>. */
  cacheKey?: string;

  /** Extra SVG drawn inside a node's group, in node-local coordinates (origin at the centre). */
  renderNodeDecor?: (node: VizNode, box: VizNodeBox) => ReactNode;
  classNames?: GraphClassNames;

  /** The graph's accessible name: its summary ("7 agents, 6 connections"). */
  ariaLabel: string;
  testIds?: GraphTestIds;
  /** Derives the default test ids: <prefix>-node:<id>, <prefix>-edge:<from>:<to>:<relation>, <prefix>-legend, ... */
  testIdPrefix?: string;
  className?: string;
  rootAttributes?: VizAttributes;
  style?: CSSProperties;
  /** Fixed height; else the view fills its container. */
  height?: number | string;
  onViewportChange?: (viewport: VizViewport) => void;
  /** Called once, after the first frame is painted, with the milliseconds since mount. */
  onFirstRender?: (ms: number) => void;
}

/** Imperative handle (`ref`) for panels: return focus, reveal, fit. */
export interface GraphViewHandle {
  fit(): void;
  /** Zooms to show these nodes; a goal shows its whole frame (double-click a goal header). */
  fitTo(ids: readonly string[]): void;
  zoomBy(factor: number): void;
  reveal(id: string): void;
  /** Moves keyboard focus to the node (the drawer's Escape returns focus here). */
  focusNode(id: string): void;
  viewport(): VizViewport;
}
