// Pan and zoom for the graph (design spec A3.9): wheel zoom around the
// pointer between 0.25x and 3x, drag to pan, fit with the canvas's 32 px
// inner padding. Pure functions; GraphView owns the state. This is the one
// zoom-pan module the console's graphs share.

import type { LayoutBounds } from "../layout/types";
import type { VizViewport } from "./types";

export const ZOOM = {
  min: 0.25,
  max: 3,
  /** One wheel notch. */
  step: 1.12,
  /** One press of + or - or a zoom button. */
  buttonStep: 1.25,
  pad: 32,
} as const;

export const IDENTITY_VIEWPORT: VizViewport = { tx: 0, ty: 0, scale: 1 };

export function clampScale(scale: number, min: number = ZOOM.min, max: number = ZOOM.max): number {
  return Math.min(max, Math.max(min, scale));
}

/** Zoom by `factor` keeping the screen point (px, py) over the same world point. */
export function zoomAt(viewport: VizViewport, factor: number, px: number, py: number): VizViewport {
  const scale = clampScale(viewport.scale * factor);
  if (scale === viewport.scale) return viewport;
  const wx = (px - viewport.tx) / viewport.scale;
  const wy = (py - viewport.ty) / viewport.scale;
  return { scale, tx: px - wx * scale, ty: py - wy * scale };
}

/** Zoom to an exact scale around the screen point (px, py). */
export function zoomTo(viewport: VizViewport, scale: number, px: number, py: number): VizViewport {
  return zoomAt(viewport, clampScale(scale) / viewport.scale, px, py);
}

export interface FitLimits {
  /** Never shrink below this (cards keep 10 px titles; marks keep their spacing). */
  minScale: number;
  /** Never enlarge beyond this. */
  maxScale: number;
  /** Pin a short picture to the top instead of centring it (layered reads like a document). */
  alignTop?: boolean;
}

/** Room to keep clear at each side of the frame, in px. */
export interface FitPadding {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

/**
 * The view that shows `bounds` in a `width` x `height` frame: as large as
 * fits within the limits, centred in the padded frame on any axis that fits,
 * pinned to the top-left on an axis that does not (a big graph opens
 * readable, then pans). The padding keeps content clear of the chrome.
 */
export function fitViewport(
  bounds: LayoutBounds,
  width: number,
  height: number,
  limits: FitLimits,
  pad: number | FitPadding = ZOOM.pad,
): VizViewport {
  if (width <= 0 || height <= 0 || bounds.w <= 0 || bounds.h <= 0) return IDENTITY_VIEWPORT;
  const p = typeof pad === "number" ? { top: pad, right: pad, bottom: pad, left: pad } : pad;
  const innerW = Math.max(1, width - p.left - p.right);
  const innerH = Math.max(1, height - p.top - p.bottom);
  const contain = Math.min(innerW / bounds.w, innerH / bounds.h);
  const scale = Math.max(limits.minScale, Math.min(limits.maxScale, contain));
  const slackX = innerW - bounds.w * scale;
  const slackY = innerH - bounds.h * scale;
  const tx = p.left + (slackX >= 0 ? slackX / 2 : 0) - bounds.x * scale;
  const ty = p.top + (slackY >= 0 && !limits.alignTop ? slackY / 2 : 0) - bounds.y * scale;
  return { tx, ty, scale };
}

/** A box in layout coordinates, (x, y) its centre. */
export interface RevealBox {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * The smallest pan that brings `box` inside the frame with `margin` to
 * spare; a box too large for the frame shows its top-left corner. The same
 * viewport object comes back when the box is already in view.
 */
export function revealViewport(viewport: VizViewport, box: RevealBox, width: number, height: number, margin = 24): VizViewport {
  const axis = (t: number, start: number, size: number, extent: number): number => {
    const low = t + start * viewport.scale;
    const high = t + (start + size) * viewport.scale;
    if (high - low + 2 * margin > extent || low < margin) return t + (margin - low);
    if (high > extent - margin) return t - (high - (extent - margin));
    return t;
  };
  const tx = axis(viewport.tx, box.x - box.w / 2, box.w, width);
  const ty = axis(viewport.ty, box.y - box.h / 2, box.h, height);
  return tx === viewport.tx && ty === viewport.ty ? viewport : { ...viewport, tx, ty };
}

/** Centre the world point (wx, wy) in the frame at the current scale. */
export function centreOn(viewport: VizViewport, wx: number, wy: number, width: number, height: number): VizViewport {
  return { ...viewport, tx: width / 2 - wx * viewport.scale, ty: height / 2 - wy * viewport.scale };
}

/** The world rectangle the frame shows. */
export function visibleWorld(viewport: VizViewport, width: number, height: number): LayoutBounds {
  return { x: -viewport.tx / viewport.scale, y: -viewport.ty / viewport.scale, w: width / viewport.scale, h: height / viewport.scale };
}

/** Whether the content overflows the frame by 20 % at this zoom. */
export function contentOverflows(bounds: LayoutBounds, viewport: VizViewport, width: number, height: number): boolean {
  if (width <= 0 || height <= 0) return false;
  return bounds.w * viewport.scale > width * 1.2 || bounds.h * viewport.scale > height * 1.2;
}

/**
 * The minimap's cue (A3.9): the content overflows the frame by 20 %, or the
 * view is zoomed in past what shows it all (part of it lies out of view).
 */
export function needsMinimap(bounds: LayoutBounds, viewport: VizViewport, width: number, height: number, slack = 24): boolean {
  if (width <= 0 || height <= 0 || bounds.w <= 0 || bounds.h <= 0) return false;
  if (contentOverflows(bounds, viewport, width, height)) return true;
  const x0 = bounds.x * viewport.scale + viewport.tx;
  const y0 = bounds.y * viewport.scale + viewport.ty;
  const x1 = x0 + bounds.w * viewport.scale;
  const y1 = y0 + bounds.h * viewport.scale;
  return x0 < -slack || y0 < -slack || x1 > width + slack || y1 > height + slack;
}

export function viewportTransform(viewport: VizViewport): string {
  return `translate(${round(viewport.tx)} ${round(viewport.ty)}) scale(${round(viewport.scale, 4)})`;
}

function round(value: number, digits = 2): number {
  const k = 10 ** digits;
  return Math.round(value * k) / k;
}

/** Wheel deltas in pixels, whatever the event's delta mode. */
export function wheelPixels(event: Pick<WheelEvent, "deltaX" | "deltaY" | "deltaMode">): { dx: number; dy: number } {
  const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? 400 : 1;
  return { dx: event.deltaX * unit, dy: event.deltaY * unit };
}

/** The zoom factor for a wheel event: one step per notch, smooth for trackpads. */
export function wheelZoomFactor(event: Pick<WheelEvent, "deltaY" | "deltaMode" | "ctrlKey">): number {
  const { dy } = wheelPixels({ deltaX: 0, deltaY: event.deltaY, deltaMode: event.deltaMode });
  if (dy === 0) return 1;
  // A notch is about 100 px; pinch gestures arrive as small ctrl-wheel deltas.
  const notches = Math.max(-4, Math.min(4, -dy / (event.ctrlKey ? 25 : 100)));
  return ZOOM.step ** notches;
}
