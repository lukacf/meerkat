import type { ConsoleWorkbenchTarget } from "@console-core";

export interface PanelRuntime {
  key: string;
  label: string;
  namespace?: string;
  memory: boolean;
  workgraph: boolean;
}

/** The host's authenticated storage scope, shared with console preferences. */
function selectionStorageKey(scope: string): string {
  return `meta-console-panel-runtime:${encodeURIComponent(scope)}`;
}

export function loadPanelRuntimeSelection(storage: Pick<Storage, "getItem"> | null, scope: string | null): string {
  if (!scope) return "";
  try { return storage?.getItem(selectionStorageKey(scope)) || ""; }
  catch { return ""; }
}

export function savePanelRuntimeSelection(storage: Pick<Storage, "setItem"> | null, scope: string | null, key: string): void {
  if (!scope || !key) return;
  try { storage?.setItem(selectionStorageKey(scope), key); }
  catch { /* The current selection remains usable when preferences cannot be saved. */ }
}

/** An explicit selection stays explicit if its runtime later disappears. */
export function selectedPanelRuntime(selected: string, runtimes?: PanelRuntime[]): string {
  return selected || runtimes?.[0]?.key || "";
}

export function scopedPanelParams(
  kind: ConsoleWorkbenchTarget["kind"],
  runtimeKey: string,
  params?: Record<string, unknown>,
): Record<string, unknown> | undefined {
  if (!runtimeKey || (kind !== "meta/memory" && kind !== "meta/workgraph")) return params;
  return { runtime_key: runtimeKey, ...params };
}

/** Unknown provenance cannot be shown as evidence for a selected runtime. */
export function framesForPanelRuntime<T extends { runtimeKey?: string }>(
  frames: T[],
  runtimeKey?: string,
): T[] {
  return runtimeKey === undefined ? frames : frames.filter(frame => frame.runtimeKey === runtimeKey);
}
