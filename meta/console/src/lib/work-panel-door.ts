// The docked Work panel shows the work graph of the launch mob's door and
// names no row. A work item is reassigned within the mob whose graph the
// panel shows: the page refuses an assignee behind another door before any
// request, in words that say why.

/** How a row key is addressed: the mob whose door serves it, or null for the launch mob's door. */
export type WorkPanelDoor = (rowKey: string) => { mobId: string | null };

/**
 * What the page says in place of a reassign of the docked Work panel to
 * `identity`, or null when the reassign is sent. `team` is the name for
 * people of the team whose work the panel shows. An agent behind a door of
 * its own is an agent of another team; a name the page does not know is
 * left to the gateway, which answers for the panel's own team.
 */
export function workPanelReassignRefusal(identity: string, door: WorkPanelDoor, team: string): string | null {
  if (door(identity).mobId === null) return null;
  return `The Work panel shows the work of ${team}. ${identity} is an agent of another team, and a work item is reassigned within its own team. Nothing was sent.`;
}
