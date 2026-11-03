import type { HostWorkbenchTarget } from "./targets";
import type { ConsoleExperience } from "./runtime-types";

export type ConsoleJsonValue = null | boolean | number | string | ConsoleJsonValue[] | { [key: string]: ConsoleJsonValue };
export interface ConsoleConversationTarget { scopeKey: string; identity: string }
export interface ConsolePanelOpenOptions {
  instanceKey?: string;
  /** Host authority scope for persisted application-wide panels. */
  scopeKey?: string;
  params?: ConsoleJsonValue;
  conversation?: ConsoleConversationTarget | null;
  followSelection?: boolean;
  intent?: "replace_focused" | "new_tab" | "split_right" | "split_down";
}
export interface ConsolePanelRequest {
  path: string;
  method?: "GET" | "POST" | "PUT" | "PATCH" | "DELETE";
  body?: ConsoleJsonValue;
}
export interface ConsolePanelScope {
  key: string;
  runtimeId?: string;
}
export type ConsolePanelService = (
  request: ConsolePanelRequest,
  scope: { authority: ConsolePanelScope; conversation: ConsoleConversationTarget | null; readOnly: boolean },
  signal: AbortSignal,
) => Promise<unknown>;

export interface ConsolePanelContext {
  baseUrl: string;
  readOnly: boolean;
  experience: ConsoleExperience | null;
  authority: ConsolePanelScope;
  visibleIdentities: readonly string[];
  selection: ConsoleConversationTarget | null;
  conversation: ConsoleConversationTarget | null;
  /** Supplied for panel mounts, never a runtime authorization claim. */
  panel?: { instanceKey: string; params: ConsoleJsonValue; focused: boolean };
  request: (request: ConsolePanelRequest, signal: AbortSignal, conversation?: ConsoleConversationTarget | null) => Promise<unknown>;
  openPanel: (id: string, options?: ConsolePanelOpenOptions | ConsolePanelOpenOptions["intent"]) => void;
}

/** A mount owns only its container. Abort work and release resources on dispose. */
export type ConsolePanelMount<T> = (
  container: HTMLElement, context: T, signal: AbortSignal,
) => { update?: (context: T) => void; dispose: () => void } | void;

export interface ConsolePanelDefinition {
  id: string;
  title: string;
  mount: ConsolePanelMount<ConsolePanelContext>;
  /** Required when opening a panel with non-null parameters. Must be pure. */
  validateParams?: (params: ConsoleJsonValue) => boolean;
}

export interface ConsoleCustomPanelTarget extends HostWorkbenchTarget<ConsoleCustomPanelPayload> {
  kind: "custom/panel";
  payload: ConsoleCustomPanelPayload;
}

export interface ConsoleCustomPanelPayload {
  panelId: string;
  scopeKey: string | null;
  instanceKey: string;
  params: ConsoleJsonValue;
  conversation: ConsoleConversationTarget | null;
  followSelection: boolean;
}

export function consoleCustomPanelTarget(panel: ConsolePanelDefinition, options: ConsolePanelOpenOptions = {}): ConsoleCustomPanelTarget {
  const instanceKey = options.instanceKey ?? "default";
  if (typeof instanceKey !== "string" || !instanceKey.trim() || instanceKey.length > 512) throw new Error("Invalid panel instance key");
  // Round-trip only plain JSON selectors. Functions, cycles, nonfinite numbers and secrets do not belong in layout preferences.
  const params = options.params ?? null;
  assertConsoleJson(params);
  if (params !== null && (!panel.validateParams || !panel.validateParams(params))) throw new Error(`Invalid parameters for ${panel.id}`);
  const conversation = options.conversation ?? null;
  if (conversation && (typeof conversation.identity !== "string" || !conversation.identity.trim() || typeof conversation.scopeKey !== "string" || !conversation.scopeKey.trim())) throw new Error("Invalid conversation target");
  return { id: `custom-panel:${panel.id}:${instanceKey}`, kind: "custom/panel", title: panel.title, payloadVersion: 1, provenance: "host",
    payload: { panelId: panel.id, scopeKey: options.scopeKey ?? conversation?.scopeKey ?? null, instanceKey, params: JSON.parse(JSON.stringify(params)),
      conversation: conversation ? { ...conversation } : null, followSelection: options.followSelection === true } };
}

function assertConsoleJson(value: unknown, ancestors = new Set<unknown>()): asserts value is ConsoleJsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean" || (typeof value === "number" && Number.isFinite(value))) return;
  if (typeof value !== "object" || ancestors.has(value) || (!Array.isArray(value) && Object.getPrototypeOf(value) !== Object.prototype)) {
    throw new Error("Panel parameters must be plain JSON");
  }
  ancestors.add(value);
  for (const item of Object.values(value)) assertConsoleJson(item, ancestors);
  ancestors.delete(value);
}

const namespaced = (value: unknown): value is string => typeof value === "string"
  && /^[a-zA-Z0-9][a-zA-Z0-9._-]*\/[a-zA-Z0-9][a-zA-Z0-9._-]*$/.test(value)
  && !value.startsWith("meta/") && !value.startsWith("meta/");
/** Validate a complete set before publishing it; duplicate registrations are errors. */
export function validateConsolePanels(panels: readonly ConsolePanelDefinition[]): void {
  const ids = new Set<string>();
  for (const panel of panels) {
    if (!panel || !namespaced(panel.id) || ids.has(panel.id) || !panel.title?.trim() || typeof panel.mount !== "function"
      || (panel.validateParams !== undefined && typeof panel.validateParams !== "function")) {
      throw new Error(`Invalid or duplicate custom panel: ${panel?.id}`);
    }
    ids.add(panel.id);
  }
}

/** Only runtime-configured modules on the console server's origin may execute. */
export function consolePanelModuleUrl(path: string, baseUrl: string): string {
  const base = new URL(baseUrl || "/", globalThis.location?.href ?? "http://localhost/");
  const url = new URL(path, `${base.href.replace(/\/$/, "")}/`);
  if (!path.trim() || !["http:", "https:"].includes(url.protocol) || url.origin !== base.origin
    || url.username || url.password || url.hash) throw new Error("Custom panel modules must use same-origin HTTP URLs");
  return url.href;
}
