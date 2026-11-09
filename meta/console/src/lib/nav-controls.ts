// Which workbench views the console offers (the sidebar's nav buttons and
// the pane menu's views), from what the runtime and the configuration allow.
// The runtime decides what exists; the configuration only narrows it.

import { normalizeNavKind, type NavKind } from "../panels/Sidebar";

export interface ConsoleNavControlsInput {
  /** The runtime has a team control surface (not the shared console). */
  hasMobControlSurface: boolean;
  /** `console_config.sidebar`: `visible_controls` lists the stock views to
   * show (it wins), else `hidden_controls` lists the ones to hide. */
  sidebar?: { visible_controls?: string[]; hidden_controls?: string[] } | null;
  access?: { can_administer?: boolean } | null;
  memory?: { available?: boolean; can_read?: boolean } | null;
  workgraph?: { available?: boolean; can_view?: boolean } | null;
  /** An explicitly empty shared panel registry: a setup state. */
  emptyPanelRegistry: boolean;
  /** The gateway lists console schedules (`meta/schedule/list`). */
  schedulesListed: boolean;
}

/**
 * The views the console offers, in nav order:
 * - the stock views the runtime has (gating and routing only with a team
 *   control surface), narrowed by the configuration;
 * - Access for administrators only, whatever the configuration says (the
 *   server gates it per principal);
 * - Memory when the caller may read it, and on a team runtime without a
 *   memory store as a "not set up" entry, so the view explains how to get
 *   memory instead of being missing;
 * - WorkGraph when the caller may view it;
 * - Schedules when the gateway lists schedules.
 * An empty shared panel registry keeps Memory and WorkGraph as setup
 * explanations. `hidden_controls` hides Memory and Schedules too.
 */
export function consoleNavControls(input: ConsoleNavControlsInput): NavKind[] {
  const runtimeControls: NavKind[] = input.hasMobControlSurface
    ? ["topology", "timeline", "gating", "roster", "routing", "logs", "health"]
    : ["topology", "timeline", "roster", "logs", "health"];
  const allowedByRuntime = new Set(runtimeControls);
  const configuredVisible = (input.sidebar?.visible_controls || [])
    .map(normalizeNavKind)
    .filter((kind): kind is NavKind => Boolean(kind) && allowedByRuntime.has(kind!));
  const hidden = new Set(
    configuredVisible.length > 0
      ? []
      : (input.sidebar?.hidden_controls || []).map(normalizeNavKind).filter((kind): kind is NavKind => Boolean(kind)),
  );
  const memory = input.memory;
  const memoryNotSetUp = input.hasMobControlSurface && memory?.available === false;
  const extras: NavKind[] = [];
  if (input.access?.can_administer === true) extras.push("access");
  if ((input.emptyPanelRegistry || memory?.can_read === true || memoryNotSetUp) && !hidden.has("memory")) {
    extras.push("memory");
  }
  if (input.emptyPanelRegistry || (input.workgraph?.available === true && input.workgraph?.can_view === true)) {
    extras.push("workgraph");
  }
  if (input.schedulesListed && !hidden.has("schedules")) extras.push("schedules");
  const stock = configuredVisible.length > 0 ? configuredVisible : runtimeControls.filter((kind) => !hidden.has(kind));
  return [...stock, ...extras];
}
