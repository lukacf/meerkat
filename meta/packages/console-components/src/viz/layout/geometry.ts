// Mark and card geometry (design spec A3.9, sizes at zoom 1) and label
// measurement. Layouts space nodes by it; renderers draw by it.

import type { VizNode, VizNodeKind } from "../graph/types";

/** A mark's radius and the radius its rings and badges reach. */
export interface MarkGeometry {
  /** The mark itself (half the square's side, half the cylinder's width). */
  r: number;
  /** Including the coordinator ring and the external dashed ring. */
  outer: number;
}

const MARKS: Record<Exclude<VizNodeKind, "item" | "goal">, MarkGeometry> = {
  agent: { r: 6, outer: 6 },
  coordinator: { r: 6, outer: 9 },
  helper: { r: 4.5, outer: 4.5 },
  person: { r: 5.5, outer: 5.5 },
  source: { r: 4.5, outer: 4.5 },
  store: { r: 7, outer: 7 },
  external: { r: 5, outer: 8 },
  unknown: { r: 5, outer: 5 },
};

const SIZE_SCALE = { sm: 0.8, md: 1, lg: 1.3 } as const;

/** The dots of the clusters layout (the brand's engineering picture). */
export const CLUSTER_DOT = { filled: 2.6, hollow: 2.4 } as const;

export function isCardKind(kind: VizNodeKind): kind is "item" | "goal" {
  return kind === "item" || kind === "goal";
}

export function markGeometry(kind: VizNodeKind, size: VizNode["size"] = "md"): MarkGeometry {
  if (isCardKind(kind)) return { r: 6, outer: 6 };
  const base = MARKS[kind] ?? MARKS.agent;
  const scale = SIZE_SCALE[size ?? "md"] ?? 1;
  return { r: base.r * scale, outer: base.outer * scale };
}

export type Density = "comfortable" | "compact";

/** The work item card (A4.3): 216 x 48, compact 168 x 36. */
export function itemCardSize(density: Density): { w: number; h: number } {
  return density === "compact" ? { w: 168, h: 36 } : { w: 216, h: 48 };
}

/** Height of a frame's header strip (the goal's own row). */
export function frameHeaderHeight(density: Density): number {
  return density === "compact" ? 28 : 34;
}

export const LABEL_FONT_SIZE = 11;
export const LABEL_LINE = 13;
/** Gap between a mark's outer edge and its label. */
export const LABEL_GAP = 5;
/** Average advance of Inter Tight 11 px / 500 and 10 px caps tracked 0.1em, for when nothing can measure. */
const LABEL_EM = 0.56;
const LANE_CHAR = 7.4;

export type TextMeasure = (text: string) => number;

/** A width estimate for 11 px labels where no canvas can measure (tests, server render). */
export const estimateLabelWidth: TextMeasure = (text) => Math.ceil(text.length * LABEL_FONT_SIZE * LABEL_EM);

/** A width estimate for the 10 px tracked caps of lane and frame labels. */
export function estimateLaneLabelWidth(text: string): number {
  return Math.ceil(text.length * LANE_CHAR);
}

/**
 * A label measurer for `font` (a canvas font shorthand), cached per text.
 * Falls back to the estimate where no 2D canvas exists (jsdom).
 */
export function createTextMeasure(font: string, doc: Document | null = typeof document === "undefined" ? null : document): TextMeasure {
  let context: CanvasRenderingContext2D | null = null;
  try {
    context = doc ? doc.createElement("canvas").getContext("2d") : null;
  } catch {
    context = null;
  }
  if (!context) return estimateLabelWidth;
  const ctx = context;
  const cache = new Map<string, number>();
  return (text) => {
    const hit = cache.get(text);
    if (hit !== undefined) return hit;
    ctx.font = font;
    const width = Math.ceil(ctx.measureText(text).width);
    cache.set(text, width);
    return width;
  };
}

/** The box a node takes in layout: a card, or a mark's diameter. */
export function nodeBoxSize(node: Pick<VizNode, "kind" | "size">, density: Density): { w: number; h: number } {
  if (node.kind === "item") return itemCardSize(density);
  if (node.kind === "goal") return { w: itemCardSize(density).w, h: frameHeaderHeight(density) };
  const { outer } = markGeometry(node.kind, node.size);
  return { w: outer * 2, h: outer * 2 };
}
