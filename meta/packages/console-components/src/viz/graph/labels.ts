// Label plates (design spec A3.9): 11 px labels on a plate-coloured halo,
// below the node; on a collision they try right, left, then above; a label
// that still collides is dropped at this zoom unless its node is selected,
// hovered, focused or a hub. Placement runs in screen pixels, so it depends
// on the zoom; labels keep their size while marks spread or gather.

export type LabelSide = "below" | "right" | "left" | "above";

export interface LabelInput {
  id: string;
  /** Label width in px. */
  width: number;
  /** The node centre in screen px (world times scale; any translation). */
  x: number;
  y: number;
  /** The mark's outer radius in screen px. */
  r: number;
  /** Placed first when higher. */
  priority: number;
  /** Placed even when it collides (selected, hovered, focused, hub). */
  force: boolean;
  /** Sides to try, in order; defaults below, right, left, above. */
  sides?: readonly LabelSide[];
}

export interface LabelPlacement {
  /** Text anchor point relative to the node centre, screen px (baseline). */
  dx: number;
  dy: number;
  anchor: "start" | "middle" | "end";
  side: LabelSide;
}

export interface Obstacle {
  id: string;
  x: number;
  y: number;
  r: number;
}

export const LABEL_HEIGHT = 13;
export const LABEL_GAP = 5;
const HALO = 2;
const CELL = 40;

interface Rect {
  x0: number;
  y0: number;
  x1: number;
  y1: number;
  owner: string;
}

class Grid {
  private readonly cells = new Map<string, Rect[]>();

  add(rect: Rect): void {
    for (const key of this.keys(rect)) {
      const list = this.cells.get(key);
      if (list) list.push(rect);
      else this.cells.set(key, [rect]);
    }
  }

  hits(rect: Rect): boolean {
    for (const key of this.keys(rect)) {
      for (const other of this.cells.get(key) ?? []) {
        if (other.owner === rect.owner) continue;
        if (other.x0 < rect.x1 && other.x1 > rect.x0 && other.y0 < rect.y1 && other.y1 > rect.y0) return true;
      }
    }
    return false;
  }

  private *keys(rect: Rect): Generator<string> {
    const cx0 = Math.floor(rect.x0 / CELL);
    const cx1 = Math.floor(rect.x1 / CELL);
    const cy0 = Math.floor(rect.y0 / CELL);
    const cy1 = Math.floor(rect.y1 / CELL);
    for (let cx = cx0; cx <= cx1; cx += 1) for (let cy = cy0; cy <= cy1; cy += 1) yield `${cx},${cy}`;
  }
}

const DEFAULT_SIDES: readonly LabelSide[] = ["below", "right", "left", "above"];

/** The sides to try for a label at `angle` around a radial centre: outward first. */
export function outwardSides(angle: number): LabelSide[] {
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const first: LabelSide = c > 0.38 ? "right" : c < -0.38 ? "left" : s < 0 ? "above" : "below";
  return [first, ...DEFAULT_SIDES.filter((side) => side !== first)];
}

/** A placement on one side of a node, and the rectangle its plate covers. */
export function labelAt(side: LabelSide, input: Pick<LabelInput, "width" | "r">): { placement: LabelPlacement; box: [number, number, number, number] } {
  const gap = input.r + LABEL_GAP;
  const w = input.width;
  switch (side) {
    case "right":
      return { placement: { dx: gap, dy: 4, anchor: "start", side }, box: [gap, -LABEL_HEIGHT / 2, w, LABEL_HEIGHT] };
    case "left":
      return { placement: { dx: -gap, dy: 4, anchor: "end", side }, box: [-gap - w, -LABEL_HEIGHT / 2, w, LABEL_HEIGHT] };
    case "above":
      return { placement: { dx: 0, dy: -gap - 3, anchor: "middle", side }, box: [-w / 2, -gap - LABEL_HEIGHT, w, LABEL_HEIGHT] };
    case "below":
    default:
      return { placement: { dx: 0, dy: gap + 9.5, anchor: "middle", side: "below" }, box: [-w / 2, gap, w, LABEL_HEIGHT] };
  }
}

/**
 * Places labels greedily by priority against the marks and the labels
 * already placed. Returns the placed labels; the missing ones were dropped.
 */
export function placeLabels(inputs: readonly LabelInput[], obstacles: readonly Obstacle[]): Map<string, LabelPlacement> {
  const grid = new Grid();
  for (const mark of obstacles) {
    grid.add({ x0: mark.x - mark.r, y0: mark.y - mark.r, x1: mark.x + mark.r, y1: mark.y + mark.r, owner: mark.id });
  }
  const placed = new Map<string, LabelPlacement>();
  const order = [...inputs].sort((a, b) => b.priority - a.priority || (a.id < b.id ? -1 : 1));
  for (const input of order) {
    const sides = input.sides ?? DEFAULT_SIDES;
    let chosen: { placement: LabelPlacement; rect: Rect } | null = null;
    for (const side of sides) {
      const { placement, box } = labelAt(side, input);
      const rect: Rect = {
        x0: input.x + box[0] - HALO,
        y0: input.y + box[1] - HALO / 2,
        x1: input.x + box[0] + box[2] + HALO,
        y1: input.y + box[1] + box[3] + HALO / 2,
        owner: input.id,
      };
      if (!grid.hits(rect)) {
        chosen = { placement, rect };
        break;
      }
    }
    if (!chosen && input.force) {
      const { placement, box } = labelAt(sides[0], input);
      chosen = {
        placement,
        rect: { x0: input.x + box[0], y0: input.y + box[1], x1: input.x + box[0] + box[2], y1: input.y + box[1] + box[3], owner: input.id },
      };
    }
    if (!chosen) continue;
    grid.add(chosen.rect);
    placed.set(input.id, chosen.placement);
  }
  return placed;
}

/** Truncates `text` with an ellipsis to fit `maxWidth` by the measure. */
export function fitText(text: string, maxWidth: number, measure: (text: string) => number): string {
  if (maxWidth <= 0) return "";
  if (measure(text) <= maxWidth) return text;
  let lo = 0;
  let hi = text.length;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (measure(`${text.slice(0, mid).trimEnd()}…`) <= maxWidth) lo = mid;
    else hi = mid - 1;
  }
  return lo > 0 ? `${text.slice(0, lo).trimEnd()}…` : "…";
}
