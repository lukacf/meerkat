import React from "react";
import {
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
} from "../../../packages/console-core/src/sidebar-preferences";

/**
 * Per-user browser preferences: the display settings, the sidebar's and the
 * activity rail's collapse, and the roster's section order and collapse.
 *
 * A preference is kept per preference scope, the signed-in caller's storage
 * scope: `<name>:<scope>`. Until the console knows its scope (its experience
 * has not loaded yet), the scope this browser used last stands in, so a
 * browser that one person uses shows no change when the scope lands. A value
 * an earlier console kept for everyone (`<name>` alone) is read once: the
 * first scope that has none of its own adopts it, and the shared copy goes.
 */
export const LAST_PREFERENCE_SCOPE_KEY = "meta-console-preference-scope:v1";

export interface PreferenceStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export const scopedPreferenceKey = (name: string, scope: string): string => `${name}:${scope}`;

/** The scope this browser's console used last, if any. */
export function lastPreferenceScope(storage: Pick<PreferenceStorage, "getItem"> | null): string | null {
  try {
    return storage?.getItem(LAST_PREFERENCE_SCOPE_KEY)?.trim() || null;
  } catch {
    return null;
  }
}

/** Remembers the scope the console uses now, for the next console load. */
export function rememberPreferenceScope(storage: PreferenceStorage | null, scope: string): void {
  try {
    if (storage?.getItem(LAST_PREFERENCE_SCOPE_KEY) !== scope) storage?.setItem(LAST_PREFERENCE_SCOPE_KEY, scope);
  } catch {
    // Storage denied: the next load reads the shared values.
  }
}

/** Moves an earlier console's shared value to `scopedKey` once, keeping a
 * value the scope already has. */
function adoptShared(storage: PreferenceStorage, sharedKey: string, scopedKey: string): void {
  const shared = storage.getItem(sharedKey);
  if (shared === null) return;
  if (storage.getItem(scopedKey) === null) storage.setItem(scopedKey, shared);
  storage.removeItem(sharedKey);
}

/**
 * The stored value of preference `name` for `scope`; `null` when there is
 * none. While the scope is unknown (`null`), the last scope's value, else an
 * earlier console's shared value, stands in.
 */
export function readPreference(storage: PreferenceStorage | null, name: string, scope: string | null): string | null {
  if (!storage) return null;
  try {
    if (scope !== null) {
      adoptShared(storage, name, scopedPreferenceKey(name, scope));
      return storage.getItem(scopedPreferenceKey(name, scope));
    }
    const last = lastPreferenceScope(storage);
    return storage.getItem(last ? scopedPreferenceKey(name, last) : name);
  } catch {
    return null;
  }
}

/** The key a choice is written under: its scope's, or the last scope's while
 * the scope is unknown. `null` when neither is known: the choice then holds
 * for this page only. */
export function preferenceWriteKey(storage: PreferenceStorage | null, name: string, scope: string | null): string | null {
  const effective = scope ?? lastPreferenceScope(storage);
  return effective ? scopedPreferenceKey(name, effective) : null;
}

export function writePreference(storage: PreferenceStorage | null, name: string, scope: string | null, value: string): void {
  const key = preferenceWriteKey(storage, name, scope);
  if (!storage || !key) return;
  try {
    storage.setItem(key, value);
  } catch {
    // Storage full or denied: the choice holds for this page.
  }
}

function browserPreferenceStorage(): PreferenceStorage | null {
  try {
    return typeof window !== "undefined" ? window.localStorage : null;
  } catch {
    return null;
  }
}

/** A per-user preference as React state: its stored value for `scope`
 * (`null` when the user made no choice), and a setter that keeps a choice. */
export function useBrowserPreference(name: string, scope: string | null): [string | null, (value: string) => void] {
  const [state, setState] = React.useState(() => ({ scope, value: readPreference(browserPreferenceStorage(), name, scope) }));
  let value = state.value;
  if (state.scope !== scope) {
    // Another user's scope: read their value before this render shows any.
    value = readPreference(browserPreferenceStorage(), name, scope);
    setState({ scope, value });
  }
  const choose = React.useCallback((next: string) => {
    writePreference(browserPreferenceStorage(), name, scope, next);
    setState({ scope, value: next });
  }, [name, scope]);
  return [value, choose];
}

/** The roster's section and subgroup order and collapse, kept per agent-list
 * namespace (`<prefix>:<namespace>`). */
const SECTION_PREFERENCE_PREFIXES = [
  SIDEBAR_SECTION_ORDER_STORAGE_PREFIX,
  SIDEBAR_SUBGROUP_ORDER_STORAGE_PREFIX,
  SECTION_COLLAPSE_STORAGE_PREFIX,
  SUBGROUP_COLLAPSE_STORAGE_PREFIX,
] as const;

/** Moves the roster's section preferences an earlier console kept for every
 * user of a runtime (`sharedNamespace`) to this user's namespace, once. */
export function adoptSharedSectionPreferences(storage: PreferenceStorage | null, sharedNamespace: string, ownNamespace: string): void {
  if (!storage || sharedNamespace === ownNamespace) return;
  try {
    for (const prefix of SECTION_PREFERENCE_PREFIXES) {
      adoptShared(storage, `${prefix}:${sharedNamespace}`, `${prefix}:${ownNamespace}`);
    }
  } catch {
    // Storage denied: the roster starts from its defaults.
  }
}
