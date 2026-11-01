import React from "react";

/**
 * The inline width of an element, kept current with a ResizeObserver
 * (0 until it is measured, and where layout is not available, as in jsdom).
 * Returns a callback ref to put on the element, so an element that mounts
 * after the component (behind a loading state) is measured too, and the
 * width, read before paint so a layout that depends on it does not flash.
 */
export function useMeasuredWidth<T extends HTMLElement>(): [(element: T | null) => void, number] {
  const [element, setElement] = React.useState<T | null>(null);
  const [width, setWidth] = React.useState(0);
  React.useLayoutEffect(() => {
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
  }, [element]);
  return [setElement, width];
}
