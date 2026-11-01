// Visualisation tokens (design spec A2.3). SVG renderers use the
// --_cc-viz-* custom properties directly in viz.css; a canvas needs concrete
// colours, so these are resolved through a probe element inside the theme
// scope and resolved again when the theme or the variant changes. Renderers
// read tokens, never hex.

import React from "react";

/** The custom property behind each token a canvas paints with. */
export const VIZ_TOKEN_PROPERTIES = {
  ink: "--_cc-viz-ink",
  mid: "--_cc-viz-mid",
  faint: "--_cc-viz-faint",
  edge: "--_cc-viz-edge",
  edgeStrong: "--_cc-viz-edge-strong",
  grid: "--_cc-viz-grid",
  plate: "--_cc-viz-plate",
  live: "--_cc-viz-live",
  liveSoft: "--_cc-viz-live-soft",
  positive: "--_cc-positive",
  warning: "--_cc-warning",
  negative: "--_cc-negative",
  meta: "--_cc-foreground-meta",
  border: "--_cc-border-soft",
  focusRing: "--_cc-focus-ring",
} as const;

export type VizTokenName = keyof typeof VIZ_TOKEN_PROPERTIES;

export type VizTokens = Readonly<Record<VizTokenName, string>> & {
  /** The display face, as the canvas font shorthand wants it. */
  readonly fontFamily: string;
};

/**
 * The rams light values, for a host without the token stylesheet and for
 * tests (which load no CSS). Each differs, so strokes stay tellable apart.
 */
export const VIZ_TOKEN_FALLBACKS: VizTokens = {
  ink: "rgb(30, 26, 20)",
  mid: "rgb(131, 129, 126)",
  faint: "rgb(205, 205, 203)",
  edge: "rgb(187, 186, 185)",
  edgeStrong: "rgb(115, 113, 110)",
  grid: "rgb(237, 237, 236)",
  plate: "rgb(255, 255, 255)",
  live: "rgb(192, 72, 21)",
  liveSoft: "rgb(244, 222, 213)",
  positive: "rgb(68, 140, 86)",
  warning: "rgb(205, 136, 0)",
  negative: "rgb(207, 64, 64)",
  meta: "rgb(101, 98, 92)",
  border: "rgb(221, 218, 214)",
  focusRing: "rgba(92, 147, 222, 0.72)",
  fontFamily: '"Inter Tight", "Inter", -apple-system, "Segoe UI", sans-serif',
};

let paintContext: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null | undefined;

function onePixelContext(doc: Document): CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null {
  if (paintContext !== undefined) return paintContext;
  try {
    if (typeof OffscreenCanvas !== "undefined") {
      paintContext = new OffscreenCanvas(1, 1).getContext("2d", { willReadFrequently: true });
    } else {
      const canvas = doc.createElement("canvas");
      canvas.width = 1;
      canvas.height = 1;
      paintContext = canvas.getContext("2d", { willReadFrequently: true });
    }
  } catch {
    paintContext = null;
  }
  return paintContext;
}

/**
 * A colour a canvas can paint: rgb() and rgba() as they are; any other form
 * (color(srgb ...), oklch(...)) painted to one pixel and read back. Null when
 * the probe resolved nothing.
 */
export function canvasColour(value: string, doc: Document = document): string | null {
  const text = value.trim();
  if (!text || text.includes("var(")) return null;
  if (/^rgba?\(/i.test(text)) return text;
  const context = onePixelContext(doc);
  if (!context) return text;
  try {
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = "rgba(0, 0, 0, 0)";
    context.fillStyle = text;
    context.fillRect(0, 0, 1, 1);
    const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data;
    return a === 255 ? `rgb(${r}, ${g}, ${b})` : `rgba(${r}, ${g}, ${b}, ${(a / 255).toFixed(3)})`;
  } catch {
    return text;
  }
}

/** Resolves every visualisation token as `scope` sees it. */
export function resolveVizTokens(scope: Element): VizTokens {
  const doc = scope.ownerDocument;
  const view = doc.defaultView;
  if (!view || typeof view.getComputedStyle !== "function") return VIZ_TOKEN_FALLBACKS;
  const probe = doc.createElement("span");
  probe.setAttribute("aria-hidden", "true");
  probe.style.position = "absolute";
  probe.style.width = "0";
  probe.style.height = "0";
  probe.style.overflow = "hidden";
  probe.style.pointerEvents = "none";
  scope.appendChild(probe);
  try {
    // An undefined token leaves `color` inherited, so read the inherited
    // colour first and treat a match as "not defined here".
    probe.style.color = "rgb(1, 2, 3)";
    const resolved: Record<string, string> = {};
    for (const name of Object.keys(VIZ_TOKEN_PROPERTIES) as VizTokenName[]) {
      probe.style.color = "rgb(1, 2, 3)";
      probe.style.setProperty("color", `var(${VIZ_TOKEN_PROPERTIES[name]}, rgb(1, 2, 3))`);
      const value = view.getComputedStyle(probe).color || "";
      const usable = value && !/^rgba?\(1, 2, 3[,)]/.test(value) ? canvasColour(value, doc) : null;
      resolved[name] = usable || VIZ_TOKEN_FALLBACKS[name];
    }
    probe.style.setProperty("font-family", "var(--_cc-font-display)");
    const family = view.getComputedStyle(probe).fontFamily;
    return { ...(resolved as Record<VizTokenName, string>), fontFamily: family || VIZ_TOKEN_FALLBACKS.fontFamily };
  } finally {
    probe.remove();
  }
}

function vizTokensEqual(a: VizTokens, b: VizTokens): boolean {
  if (a.fontFamily !== b.fontFamily) return false;
  return (Object.keys(VIZ_TOKEN_PROPERTIES) as VizTokenName[]).every((name) => a[name] === b[name]);
}

/**
 * The visualisation tokens for the element behind `ref`, resolved again
 * whenever a theme or variant attribute changes anywhere in the document.
 * Resolves nothing (and observes nothing) while `enabled` is false.
 */
export function useVizTokens(ref: React.RefObject<Element | null>, enabled = true): VizTokens {
  const [tokens, setTokens] = React.useState<VizTokens>(VIZ_TOKEN_FALLBACKS);
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!enabled || !element) return undefined;
    const resolve = () => {
      const next = resolveVizTokens(element);
      setTokens((current) => (vizTokensEqual(current, next) ? current : next));
    };
    resolve();
    if (typeof MutationObserver === "undefined") return undefined;
    const observer = new MutationObserver(resolve);
    observer.observe(element.ownerDocument.documentElement, {
      attributes: true,
      subtree: true,
      attributeFilter: ["data-cc-theme", "data-cc-variant"],
    });
    return () => observer.disconnect();
  }, [ref, enabled]);
  return tokens;
}

const REDUCED_QUERY = "(prefers-reduced-motion: reduce)";

/** Whether the person asked for reduced motion, or the console's `.reduced-motion` class is set. */
export function prefersReducedMotion(element?: Element | null): boolean {
  if (element?.closest(".reduced-motion")) return true;
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return window.matchMedia(REDUCED_QUERY).matches;
}

/** `prefersReducedMotion` as state: follows the media query and re-checks the class on every render. */
export function useReducedMotion(ref: React.RefObject<Element | null>): boolean {
  const [media, setMedia] = React.useState(() =>
    typeof window !== "undefined" && typeof window.matchMedia === "function"
      ? window.matchMedia(REDUCED_QUERY).matches
      : false,
  );
  React.useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") return undefined;
    const query = window.matchMedia(REDUCED_QUERY);
    const update = () => setMedia(query.matches);
    query.addEventListener?.("change", update);
    return () => query.removeEventListener?.("change", update);
  }, []);
  const element = ref.current;
  return media || Boolean(element?.closest(".reduced-motion"));
}
