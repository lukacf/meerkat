import React from "react";

export type ConsoleVariant = "rams" | "terminal" | "graphite";
export type ConsoleTheme = "dark" | "light";
/** The density of panels without their own choice, and of the roster. */
export type ConsoleDensity = "comfortable" | "compact";
/** Motion: the system's setting, or reduced whatever the system says. */
export type ConsoleMotion = "system" | "reduced";

/** The console's display settings (spec B21). */
export interface ConsoleDisplaySettings {
  theme: ConsoleTheme;
  variant: ConsoleVariant;
  density: ConsoleDensity;
  motion: ConsoleMotion;
}

export type ConsoleDisplayKey = keyof ConsoleDisplaySettings;

/** Where each of the user's own choices is kept. A default is never written. */
export const DISPLAY_STORAGE_KEYS: Readonly<Record<ConsoleDisplayKey, string>> = {
  theme: "meta-console-theme",
  variant: "meta-console-variant",
  density: "meta-console-density",
  motion: "meta-console-motion",
};

/** The console's own defaults, when neither the user nor the configuration chose. */
export const DEFAULT_DISPLAY: Readonly<ConsoleDisplaySettings> = Object.freeze({
  theme: "light",
  variant: "rams",
  density: "comfortable",
  motion: "system",
});

const DISPLAY_VALUES: { readonly [K in ConsoleDisplayKey]: readonly ConsoleDisplaySettings[K][] } = {
  theme: ["light", "dark"],
  variant: ["rams", "terminal", "graphite"],
  density: ["comfortable", "compact"],
  motion: ["system", "reduced"],
};

const DISPLAY_KEYS = Object.keys(DISPLAY_STORAGE_KEYS) as ConsoleDisplayKey[];

/** A display value, or null when the input is not one. */
export function normalizeDisplayValue<K extends ConsoleDisplayKey>(key: K, value: unknown): ConsoleDisplaySettings[K] | null {
  return (DISPLAY_VALUES[key] as readonly unknown[]).includes(value) ? value as ConsoleDisplaySettings[K] : null;
}

interface DisplayStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

function displayStorage(): DisplayStorage | null {
  try {
    return typeof window !== "undefined" && window.localStorage ? window.localStorage : null;
  } catch {
    return null;
  }
}

/** The user's own display choices: only the keys the user set, each a valid value. */
export function readDisplayChoices(storage: Pick<DisplayStorage, "getItem"> | null = displayStorage()): Partial<ConsoleDisplaySettings> {
  const chosen: Partial<ConsoleDisplaySettings> = {};
  for (const key of DISPLAY_KEYS) {
    let raw: string | null = null;
    try {
      raw = storage?.getItem(DISPLAY_STORAGE_KEYS[key]) ?? null;
    } catch {
      raw = null;
    }
    const value = normalizeDisplayValue(key, raw);
    if (value !== null) (chosen as Record<ConsoleDisplayKey, string>)[key] = value;
  }
  return chosen;
}

/**
 * Each setting: the user's own choice, else the configured default, else
 * the console's default.
 */
export function resolveDisplaySettings(
  chosen: Partial<ConsoleDisplaySettings>,
  configured: Partial<ConsoleDisplaySettings> = {},
): ConsoleDisplaySettings {
  return {
    theme: chosen.theme ?? configured.theme ?? DEFAULT_DISPLAY.theme,
    variant: chosen.variant ?? configured.variant ?? DEFAULT_DISPLAY.variant,
    density: chosen.density ?? configured.density ?? DEFAULT_DISPLAY.density,
    motion: chosen.motion ?? configured.motion ?? DEFAULT_DISPLAY.motion,
  };
}

export interface ConsoleDisplay {
  /** What the console shows now. */
  settings: ConsoleDisplaySettings;
  /** The user's own choices (what is stored). */
  chosen: Partial<ConsoleDisplaySettings>;
  /** Records the user's choice and applies it. */
  choose: <K extends ConsoleDisplayKey>(key: K, value: ConsoleDisplaySettings[K]) => void;
  /** Forgets the user's choices: the configured defaults apply again. */
  reset: () => void;
}

/**
 * The console's display settings (spec B21): theme, style, density and
 * motion. Only the user's own choices are stored; a configured default
 * (`appearance.default_theme`, `default_variant`) applies while the user
 * has made no choice and is never written. Other tabs follow a choice.
 */
export function useConsoleDisplay(configured: Partial<ConsoleDisplaySettings> = {}): ConsoleDisplay {
  const [chosen, setChosen] = React.useState<Partial<ConsoleDisplaySettings>>(() => readDisplayChoices());
  React.useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const keys = new Set(Object.values(DISPLAY_STORAGE_KEYS));
    const onStorage = (event: StorageEvent) => {
      if (event.key === null || keys.has(event.key)) setChosen(readDisplayChoices());
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);
  const choose = React.useCallback(<K extends ConsoleDisplayKey>(key: K, value: ConsoleDisplaySettings[K]) => {
    if (normalizeDisplayValue(key, value) === null) return;
    setChosen((current) => ({ ...current, [key]: value }));
    try {
      displayStorage()?.setItem(DISPLAY_STORAGE_KEYS[key], value);
    } catch {
      // Storage full or denied: the choice holds for this session.
    }
  }, []);
  const reset = React.useCallback(() => {
    setChosen({});
    const storage = displayStorage();
    for (const key of DISPLAY_KEYS) {
      try {
        storage?.removeItem(DISPLAY_STORAGE_KEYS[key]);
      } catch {
        // Nothing stored to forget.
      }
    }
  }, []);
  const configuredTheme = configured.theme;
  const configuredVariant = configured.variant;
  const configuredDensity = configured.density;
  const configuredMotion = configured.motion;
  const settings = React.useMemo(
    () => resolveDisplaySettings(chosen, {
      theme: configuredTheme,
      variant: configuredVariant,
      density: configuredDensity,
      motion: configuredMotion,
    }),
    [chosen, configuredDensity, configuredMotion, configuredTheme, configuredVariant],
  );
  return { settings, chosen, choose, reset };
}
