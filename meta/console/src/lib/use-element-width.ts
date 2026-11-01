import React from "react";

/**
 * The inline width of an element, kept current with a ResizeObserver
 * (0 until it is measured, and where layout is not available, as in jsdom).
 * Measured before paint, so a layout that depends on it does not flash.
 */
export function useElementWidth<T extends HTMLElement>(ref: React.RefObject<T | null>): number {
  const [width, setWidth] = React.useState(0);
  React.useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return undefined;
    const read = () => {
      const next = Math.round(element.getBoundingClientRect().width);
      setWidth((current) => (current === next ? current : next));
    };
    read();
    if (typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver(() => read());
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
