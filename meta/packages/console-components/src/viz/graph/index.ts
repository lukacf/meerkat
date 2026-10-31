// The graph kit (design spec A3.9): GraphView, its list twin, the status
// square, words, and the pieces panels and tests reach for.

export { GraphView, SVG_LIMITS } from "./graph-view";
export { GraphList, LIST_WINDOW_THRESHOLD, connectionsSummary, graphListRows } from "./list-twin";
export type { GraphListConnection, GraphListProps, GraphListRow, GraphListSort } from "./list-twin";
export { StatusSquare } from "./status-square";
export type { StatusSquareProps } from "./status-square";
export { itemStatusOf } from "./marks";
export { buildGraphModel, edgeIsDirected, highlightFor, PARALLEL_STEP } from "./model";
export type { GraphModel, GraphModelInput, Highlight } from "./model";
export { edgeMark, itemCardGeometry } from "./svg-layers";
export { nodeToward, readingOrder } from "./keyboard";
export { fitText, labelAt, outwardSides, placeLabels } from "./labels";
export type { LabelInput, LabelPlacement, LabelSide } from "./labels";
export { PARTICLES, admitMessages, easeTravel, messageKey, travelMs } from "./particles";
export {
  POSITION_CACHE_PREFIX,
  readMinimapHidden,
  readPositionCache,
  writeMinimapHidden,
  writePositionCache,
} from "./position-cache";
export {
  ZOOM,
  centreOn,
  clampScale,
  contentOverflows,
  fitViewport,
  needsMinimap,
  revealViewport,
  viewportTransform,
  visibleWorld,
  zoomAt,
  zoomTo,
} from "./zoom-pan";
export type { FitLimits, RevealBox } from "./zoom-pan";
export {
  edgeTooltipLines,
  graphSummary,
  humanizeWord,
  nodeAccessibleName,
  nodeStatusWord,
  nodeTooltipLines,
  vizKindWord,
  vizRelationWord,
} from "./words";
export type {
  GraphClassNames,
  GraphTestIds,
  GraphViewHandle,
  GraphViewProps,
  VizAttributes,
  VizEdge,
  VizEdgeState,
  VizFrame,
  VizItemStatus,
  VizLane,
  VizLayoutKind,
  VizMemoryEvent,
  VizMemoryState,
  VizMessage,
  VizNode,
  VizNodeBox,
  VizNodeKind,
  VizRelation,
  VizTone,
  VizViewport,
  VizZoomAction,
} from "./types";
