// Keyboard movement and reading order for the graph (design spec A3.9): one
// tab stop, arrow keys to the nearest node that way, Home and End to the
// first and the last node.

import type { VizNodeBox } from "./types";

const ARROWS: ReadonlyMap<string, readonly [number, number]> = new Map([
  ["ArrowRight", [1, 0]],
  ["ArrowLeft", [-1, 0]],
  ["ArrowDown", [0, 1]],
  ["ArrowUp", [0, -1]],
]);

export function isNavigationKey(key: string): boolean {
  return ARROWS.has(key) || key === "Home" || key === "End";
}

/**
 * Reading order: rows top to bottom (left to right in a row) for marks;
 * columns left to right for the layered layout, whose ranks are columns.
 */
export function readingOrder(boxes: Iterable<VizNodeBox>, columns: boolean): VizNodeBox[] {
  const list = [...boxes];
  const tolerance = 8;
  list.sort((a, b) => {
    if (columns) {
      if (Math.abs(a.x - b.x) > tolerance) return a.x - b.x;
      return a.y - b.y;
    }
    if (Math.abs(a.y - b.y) > tolerance) return a.y - b.y;
    return a.x - b.x;
  });
  return list;
}

/**
 * Where a key moves focus: an arrow goes to the nearest node ahead of the
 * current one, weighing distance across the arrow double so a press keeps to
 * its row or column; Home and End go to the first and the last node. Null
 * when no node lies that way.
 */
export function nodeToward(boxes: ReadonlyMap<string, VizNodeBox>, fromId: string | null, key: string, columns = false): string | null {
  if (key === "Home" || key === "End") {
    const order = readingOrder(boxes.values(), columns);
    const target = key === "Home" ? order[0] : order[order.length - 1];
    return target ? target.id : null;
  }
  const step = ARROWS.get(key);
  const from = step && fromId ? boxes.get(fromId) : undefined;
  if (!step || !from) return null;
  const [ux, uy] = step;
  let bestId: string | null = null;
  let bestScore = Infinity;
  for (const box of boxes.values()) {
    if (box.id === from.id) continue;
    const dx = box.x - from.x;
    const dy = box.y - from.y;
    const ahead = dx * ux + dy * uy;
    if (ahead < 1) continue;
    const score = ahead + 2 * Math.abs(dx * uy - dy * ux);
    if (score < bestScore) {
      bestScore = score;
      bestId = box.id;
    }
  }
  return bestId;
}
