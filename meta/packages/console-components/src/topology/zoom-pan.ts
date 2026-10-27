// Lightweight zoom + pan for the SVG topology layouts.
//
// Returns a viewport (tx, ty, scale) plus event handlers that hook onto
// the wrapping <svg>. The handlers translate viewport-space cursor
// coordinates to world space so wheel-zoom anchors at the cursor (not
// the SVG centre). Pan is implemented via pointerdown→move→up; cleanup
// runs on pointer cancel/leave. State is intentionally transient — no
// localStorage persistence — so a fresh view always starts at 1:1.

import React from "react";

export interface Viewport {
  tx: number;
  ty: number;
  scale: number;
}

export interface ZoomPan {
  viewport: Viewport;
  reset: () => void;
  /// Pass to the SVG element via `ref={zoom.svgRef}`. The hook attaches
  /// a non-passive wheel listener so `preventDefault` works (React's
  /// synthetic onWheel is passive in modern React/Chromium).
  svgRef: React.RefObject<SVGSVGElement | null>;
  onPointerDown: (e: React.PointerEvent<SVGSVGElement>) => void;
  onPointerMove: (e: React.PointerEvent<SVGSVGElement>) => void;
  onPointerUp: (e: React.PointerEvent<SVGSVGElement>) => void;
  isDragging: boolean;
}

const MIN_SCALE = 0.4;
const MAX_SCALE = 6;

/// Convert a clientX/Y on the <svg> element to viewBox-local coords,
/// independent of CSS scaling (`preserveAspectRatio="xMidYMid meet"`
/// fits a viewBox into a flexible-size element). Without this, zoom
/// anchors land where the cursor is on screen but not in world space.
function clientToViewBox(
  el: SVGSVGElement,
  clientX: number,
  clientY: number,
  viewBoxW: number,
  viewBoxH: number,
): { x: number; y: number } {
  const rect = el.getBoundingClientRect();
  // The board uses xMidYMid meet, so the viewBox is centred and scaled
  // uniformly to fit `rect`. Compute the actual content rectangle:
  const renderScale = Math.min(rect.width / viewBoxW, rect.height / viewBoxH);
  const contentW = viewBoxW * renderScale;
  const contentH = viewBoxH * renderScale;
  const offsetX = rect.left + (rect.width - contentW) / 2;
  const offsetY = rect.top + (rect.height - contentH) / 2;
  return {
    x: (clientX - offsetX) / renderScale,
    y: (clientY - offsetY) / renderScale,
  };
}

export function useZoomPan(width: number, height: number): ZoomPan {
  const [viewport, setViewport] = React.useState<Viewport>({ tx: 0, ty: 0, scale: 1 });
  const dragRef = React.useRef<{ pointerId: number; lastX: number; lastY: number } | null>(null);
  const [isDragging, setIsDragging] = React.useState(false);
  const svgRef = React.useRef<SVGSVGElement | null>(null);

  const reset = React.useCallback(() => {
    setViewport({ tx: 0, ty: 0, scale: 1 });
  }, []);

  React.useEffect(() => {
    setViewport({ tx: 0, ty: 0, scale: 1 });
    dragRef.current = null;
    setIsDragging(false);
  }, [width, height]);

  // Attach the wheel listener manually with passive=false so we can
  // preventDefault and stop the page from scrolling while zooming.
  // React's synthetic onWheel binds passive in modern versions.
  React.useEffect(() => {
    const el = svgRef.current;
    if (!el) return;
    const handler = (e: WheelEvent) => {
      e.preventDefault();
      const { x: cx, y: cy } = clientToViewBox(el, e.clientX, e.clientY, width, height);
      setViewport((prev) => {
        const factor = e.deltaY < 0 ? 1.12 : 1 / 1.12;
        const nextScale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, prev.scale * factor));
        if (nextScale === prev.scale) return prev;
        // Anchor zoom at the cursor: the world point under the cursor
        // must remain under the cursor.
        const wx = (cx - prev.tx) / prev.scale;
        const wy = (cy - prev.ty) / prev.scale;
        return {
          scale: nextScale,
          tx: cx - wx * nextScale,
          ty: cy - wy * nextScale,
        };
      });
    };
    el.addEventListener("wheel", handler, { passive: false });
    return () => el.removeEventListener("wheel", handler);
  }, [width, height]);

  const onPointerDown = React.useCallback((e: React.PointerEvent<SVGSVGElement>) => {
    // Only pan on plain left-button presses on the canvas — let nodes
    // (which sit above) handle their own clicks. We check the target's
    // tag to avoid stealing pointerdown from interactive elements.
    if (e.button !== 0) return;
    const tag = (e.target as Element | null)?.tagName?.toLowerCase();
    if (tag !== "svg" && tag !== "g" && tag !== "rect" && tag !== "line") return;
    e.currentTarget.setPointerCapture(e.pointerId);
    dragRef.current = { pointerId: e.pointerId, lastX: e.clientX, lastY: e.clientY };
    setIsDragging(true);
  }, []);

  const onPointerMove = React.useCallback((e: React.PointerEvent<SVGSVGElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    const dx = e.clientX - drag.lastX;
    const dy = e.clientY - drag.lastY;
    drag.lastX = e.clientX;
    drag.lastY = e.clientY;
    // Convert pixel delta to viewBox units. preserveAspectRatio = meet,
    // so viewBox-units-per-pixel = viewBoxW / contentW.
    const rect = e.currentTarget.getBoundingClientRect();
    const renderScale = Math.min(rect.width / width, rect.height / height);
    const vbDx = dx / renderScale;
    const vbDy = dy / renderScale;
    setViewport((prev) => ({ ...prev, tx: prev.tx + vbDx, ty: prev.ty + vbDy }));
  }, [width, height]);

  const onPointerUp = React.useCallback((e: React.PointerEvent<SVGSVGElement>) => {
    const drag = dragRef.current;
    if (!drag || drag.pointerId !== e.pointerId) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    dragRef.current = null;
    setIsDragging(false);
  }, []);

  return { viewport, reset, svgRef, onPointerDown, onPointerMove, onPointerUp, isDragging };
}

export function viewportTransform(v: Viewport): string {
  return `translate(${v.tx} ${v.ty}) scale(${v.scale})`;
}
