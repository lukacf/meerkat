import type { AgentDirectory } from "@console-components";

import { memberDoor, memberRowKey, type MobDoorTable } from "../../../packages/console-core/src/mob-door";

/**
 * The agent directory as a pane of one hosted mob's agent reads it. A door
 * names its agents as it knows them, also deep inside its answers (who
 * asked for an agent, who a schedule sends as): in that agent's pane such a
 * bare name is an agent behind the same door, never an agent of the launch
 * mob that has the same name. A key is found when it is the console's own
 * key of an agent behind this door, or the name this door knows such an
 * agent by; any other key names nobody here (it is then shown as plain
 * words and opens nothing).
 */
export function agentDirectoryBehindDoor(base: AgentDirectory, doorMobId: string, table: MobDoorTable): AgentDirectory {
  const behind = (id: string): boolean => memberDoor(id, table).mobId === doorMobId;
  return {
    size: base.size,
    nestedMobIds: base.nestedMobIds,
    // A name this door has no agent for names nobody to open.
    opensKnownOnly: true,
    plainWords: base.plainWords,
    lookup(key) {
      const trimmed = typeof key === "string" ? key.trim() : "";
      if (!trimmed) return undefined;
      const own = base.lookup(trimmed);
      if (own && behind(own.id)) return own;
      const keyed = memberRowKey({ mob_id: doorMobId, identity: trimmed }, table);
      const member = keyed ? base.lookup(keyed) : undefined;
      return member && behind(member.id) ? member : undefined;
    },
  };
}
