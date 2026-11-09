import React from "react";
import { LAST_PREFERENCE_SCOPE_KEY, preferenceWriteKey, readPreference, writePreference } from "../lib/preference-scope";

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

/** The user's own display choices: only the keys the user set, each a valid
 * value. With a `scope`, the choices kept for that signed-in user's
 * preference scope (`null` while the console does not know it yet: the last
 * scope's); without one, the browser-wide keys. */
export function readDisplayChoices(storage: DisplayStorage | null = displayStorage(), scope?: string | null): Partial<ConsoleDisplaySettings> {
  const chosen: Partial<ConsoleDisplaySettings> = {};
  for (const key of DISPLAY_KEYS) {
    let raw: string | null = null;
    try {
      raw = scope === undefined
        ? storage?.getItem(DISPLAY_STORAGE_KEYS[key]) ?? null
        : readPreference(storage, DISPLAY_STORAGE_KEYS[key], scope);
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
 *
 * `scope` is the signed-in user's preference scope (`null` while the console
 * does not know it yet): each user of a browser keeps their own choices.
 * Without it the choices are browser-wide.
 */
export function useConsoleDisplay(configured: Partial<ConsoleDisplaySettings> = {}, scope?: string | null): ConsoleDisplay {
  const [state, setState] = React.useState(() => ({ scope, chosen: readDisplayChoices(displayStorage(), scope) }));
  let chosen = state.chosen;
  if (state.scope !== scope) {
    // Another user's scope: read their choices before this render shows any.
    chosen = readDisplayChoices(displayStorage(), scope);
    setState({ scope, chosen });
  }
  React.useEffect(() => {
    if (typeof window === "undefined") return undefined;
    const names = Object.values(DISPLAY_STORAGE_KEYS);
    const onStorage = (event: StorageEvent) => {
      if (event.key === null || event.key === LAST_PREFERENCE_SCOPE_KEY || names.some((name) => event.key === name || event.key!.startsWith(`${name}:`))) {
        setState({ scope, chosen: readDisplayChoices(displayStorage(), scope) });
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, [scope]);
  const choose = React.useCallback(<K extends ConsoleDisplayKey>(key: K, value: ConsoleDisplaySettings[K]) => {
    if (normalizeDisplayValue(key, value) === null) return;
    setState((current) => ({ ...current, chosen: { ...current.chosen, [key]: value } }));
    try {
      if (scope === undefined) displayStorage()?.setItem(DISPLAY_STORAGE_KEYS[key], value);
      else writePreference(displayStorage(), DISPLAY_STORAGE_KEYS[key], scope, value);
    } catch {
      // Storage full or denied: the choice holds for this session.
    }
  }, [scope]);
  const reset = React.useCallback(() => {
    setState((current) => ({ ...current, chosen: {} }));
    const storage = displayStorage();
    for (const key of DISPLAY_KEYS) {
      try {
        const stored = scope === undefined ? DISPLAY_STORAGE_KEYS[key] : preferenceWriteKey(storage, DISPLAY_STORAGE_KEYS[key], scope);
        if (stored) storage?.removeItem(stored);
      } catch {
        // Nothing stored to forget.
      }
    }
  }, [scope]);
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
