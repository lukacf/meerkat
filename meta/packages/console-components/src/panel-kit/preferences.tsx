import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";

import { copyTextToClipboard } from "../shared";
import type { MenuEntry } from "./menu";
import type { PanelDensity } from "./panel";

// Per-panel view preferences (spec A4.3, A3.1): density and "Show technical
// IDs", persisted in localStorage under the principal-aware preference scope
// the host provides, and kept in step across panels of a kind and across
// tabs.

interface PreferenceStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

interface PanelPreferencesValue {
  /** The principal-aware preference scope ("default" without one). */
  scope: string;
  storage: PreferenceStorage | null;
}

function defaultStorage(): PreferenceStorage | null {
  try {
    return typeof window !== "undefined" && window.localStorage ? window.localStorage : null;
  } catch {
    return null;
  }
}

const PanelPreferencesContext = createContext<PanelPreferencesValue | null>(null);

/**
 * Gives every panel preference below it the host's preference scope (the
 * same principal-aware scope the sidebar's view and order use), so two
 * people on one browser keep their own density and id settings.
 */
export function PanelPreferencesProvider({
  scope,
  storage,
  children,
}: {
  scope: string | null | undefined;
  /** Defaults to localStorage. */
  storage?: PreferenceStorage | null;
  children?: ReactNode;
}) {
  return (
    <PanelPreferencesContext.Provider value={{ scope: scope?.trim() || "default", storage: storage === undefined ? defaultStorage() : storage }}>
      {children}
    </PanelPreferencesContext.Provider>
  );
}

/** The storage key of a preference: `<prefix>:<scope>`. */
export function panelPreferenceKey(prefix: string, scope: string | null | undefined): string {
  return `${prefix}:${scope?.trim() || "default"}`;
}

const CHANGE_EVENT = "cc-panel-preference";

/**
 * A string preference in localStorage under `<prefix>:<scope>`. Reads once,
 * writes on change, and follows changes made by other panels in this tab and
 * by other tabs. Values outside `allowed` read as `initial`.
 */
export function usePanelPreference<T extends string>(
  prefix: string,
  initial: T,
  allowed?: readonly T[],
): [T, (next: T) => void] {
  const context = useContext(PanelPreferencesContext);
  const storage = context ? context.storage : defaultStorage();
  const key = panelPreferenceKey(prefix, context?.scope);
  const read = useCallback((): T => {
    try {
      const raw = storage?.getItem(key);
      if (raw === null || raw === undefined) return initial;
      return !allowed || (allowed as readonly string[]).includes(raw) ? (raw as T) : initial;
    } catch {
      return initial;
    }
    // `allowed` is a constant list at every call site.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [storage, key, initial]);
  const [value, setValue] = useState<T>(read);
  useEffect(() => {
    setValue(read());
    if (typeof window === "undefined") return undefined;
    const onStorage = (event: StorageEvent) => {
      if (event.key === key) setValue(read());
    };
    const onLocal = (event: Event) => {
      if ((event as CustomEvent<string>).detail === key) setValue(read());
    };
    window.addEventListener("storage", onStorage);
    window.addEventListener(CHANGE_EVENT, onLocal);
    return () => {
      window.removeEventListener("storage", onStorage);
      window.removeEventListener(CHANGE_EVENT, onLocal);
    };
  }, [key, read]);
  const update = useCallback(
    (next: T) => {
      setValue(next);
      try {
        if (next === initial) storage?.removeItem(key);
        else storage?.setItem(key, next);
      } catch {
        // Storage full or denied: the choice holds for this session only.
      }
      if (typeof window !== "undefined") window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: key }));
    },
    [storage, key, initial],
  );
  return [value, update];
}

const DENSITIES = ["comfortable", "compact"] as const;

/** The panel kind's density (`meta-console-density:<kind>`), comfortable by
 * default. Pass it to PanelShell's `density`. */
export function usePanelDensity(kind: string): [PanelDensity, (next: PanelDensity) => void] {
  return usePanelPreference<PanelDensity>(`meta-console-density:${kind}`, "comfortable", DENSITIES);
}

const ON_OFF = ["on", "off"] as const;

/** The panel kind's "Show technical IDs" option
 * (`meta-console-tech-ids:<kind>`), off by default. Pass it to PanelShell's
 * `showTechnicalIds`. */
export function useShowTechnicalIds(kind: string): [boolean, (next: boolean) => void] {
  const [value, setValue] = usePanelPreference<"on" | "off">(`meta-console-tech-ids:${kind}`, "off", ON_OFF);
  return [value === "on", (next: boolean) => setValue(next ? "on" : "off")];
}

/**
 * The standard overflow menu of a panel header (A3.1), in the spec's order:
 * the panel's own view options first, then Density (radio), Show technical
 * IDs (checkbox), a separator, Open in split pane, Copy data as JSON and
 * Refresh now. Entries whose callback is missing are left out. Test ids are
 * `<testIdPrefix>-menu:<part>` when a prefix is given.
 */
export function standardPanelMenu({
  viewOptions = [],
  density,
  onDensityChange,
  showTechnicalIds,
  onShowTechnicalIdsChange,
  onOpenInSplit,
  getCopyData,
  onRefresh,
  testIdPrefix,
}: {
  viewOptions?: ReadonlyArray<MenuEntry | null | false | undefined>;
  density?: PanelDensity;
  onDensityChange?: (density: PanelDensity) => void;
  showTechnicalIds?: boolean;
  onShowTechnicalIdsChange?: (on: boolean) => void;
  onOpenInSplit?: () => void;
  /** The panel's current model; "Copy data as JSON" copies it (support). */
  getCopyData?: () => unknown;
  onRefresh?: () => void;
  testIdPrefix?: string;
}): MenuEntry[] {
  const id = (part: string) => (testIdPrefix ? `${testIdPrefix}-menu:${part}` : undefined);
  const entries: MenuEntry[] = viewOptions.filter((entry): entry is MenuEntry => Boolean(entry));
  if (density && onDensityChange) {
    entries.push({
      kind: "radio",
      id: "density",
      label: "Density",
      value: density,
      options: [
        { id: "comfortable", label: "Comfortable", testId: id("density:comfortable") },
        { id: "compact", label: "Compact", testId: id("density:compact") },
      ],
      onChange: (next) => onDensityChange(next === "compact" ? "compact" : "comfortable"),
    });
  }
  if (showTechnicalIds !== undefined && onShowTechnicalIdsChange) {
    entries.push({
      kind: "checkbox",
      id: "technical-ids",
      label: "Show technical IDs",
      checked: showTechnicalIds,
      onChange: onShowTechnicalIdsChange,
      testId: id("technical-ids"),
    });
  }
  const commands: MenuEntry[] = [];
  if (onOpenInSplit) commands.push({ kind: "item", id: "split", label: "Open in split pane", glyph: "split-right", onSelect: onOpenInSplit, testId: id("split") });
  if (getCopyData) {
    commands.push({
      kind: "item",
      id: "copy-json",
      label: "Copy data as JSON",
      glyph: "copy",
      onSelect: () => {
        let text = "";
        try {
          text = JSON.stringify(getCopyData(), null, 2) ?? "";
        } catch {
          text = "";
        }
        void copyTextToClipboard(text);
      },
      testId: id("copy-json"),
    });
  }
  if (onRefresh) commands.push({ kind: "item", id: "refresh", label: "Refresh now", glyph: "refresh", onSelect: onRefresh, testId: id("refresh") });
  if (commands.length && entries.length) entries.push({ kind: "separator", id: "commands" });
  return [...entries, ...commands];
}
