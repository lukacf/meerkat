// Graph layouts (design spec A3.9): pure functions from nodes and edges to
// positions in layout space. GraphView draws them; panels and tests may call
// them directly.

export { computeGraphLayout, effectiveLayoutKind, framesFor, toLayoutEdges, toLayoutNodes } from "./compute";
export type { GraphLayoutInput } from "./compute";
export { CLUSTERS, layoutClusters, ringSlots } from "./clusters";
export { FORCE, forceAnchors, forceLayoutFrom, loadD3Force, runForce, seedForce } from "./force";
export type { ForceRun } from "./force";
export {
  CLUSTER_DOT,
  createTextMeasure,
  estimateLabelWidth,
  estimateLaneLabelWidth,
  frameHeaderHeight,
  isCardKind,
  itemCardSize,
  markGeometry,
  nodeBoxSize,
} from "./geometry";
export type { Density, MarkGeometry, TextMeasure } from "./geometry";
export { LANES, OTHER_LANE, layoutLanes } from "./lanes";
export { LAYERED, layoutLayered, orderRanks, rankItems, routeBetween } from "./layered";
export { RADIAL, RELATION_ORDER, layoutRadial, radialGroupOf } from "./radial";
export { boundsOf, compareLabels, compareNodes, emptyLayout, unionBounds } from "./types";
export type {
  ClusterBundle,
  EdgeRoute,
  GraphLayout,
  LayoutBounds,
  LayoutEdge,
  LayoutFrameInput,
  LayoutLaneInput,
  LayoutNode,
  LayoutOptions,
  LayoutPoint,
  PlacedCluster,
  PlacedFrame,
  PlacedLane,
} from "./types";
