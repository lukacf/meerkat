import React from "react";

export type ConsoleVariant = "rams" | "terminal" | "graphite";
export type ConsoleTheme = "dark" | "light";

const VARIANT_STORAGE = "meta-console-variant";

export function useConsoleVariant(): [ConsoleVariant, (v: ConsoleVariant) => void] {
  const [v, setV] = React.useState<ConsoleVariant>(() => {
    try {
      const stored = localStorage.getItem(VARIANT_STORAGE);
      if (stored === "rams" || stored === "terminal" || stored === "graphite") return stored;
    } catch { /* ignore */ }
    return "rams";
  });
  const set = React.useCallback((next: ConsoleVariant) => {
    setV(next);
    try { localStorage.setItem(VARIANT_STORAGE, next); } catch { /* ignore */ }
  }, []);
  return [v, set];
}
