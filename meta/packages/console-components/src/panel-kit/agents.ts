// Agent names for panels. Panels receive identity, member and session keys
// from the runtime; people read display names. A directory maps every key
// an agent is known by to its label. Pure module (no React), so panels whose
// test bundles cannot resolve React can still resolve names. Keys outside
// the directory are named by console-core's agentDisplayTitle, the one
// naming function every surface shares (a relative import, so bundles
// without the @console-core alias resolve it).

import { identityDisplayTitle, stripAgentKeyPrefix } from "../../../console-core/src/agent-title";
import { parseNestedMemberIdentity } from "../../../console-core/src/member-ref";

export { stripAgentKeyPrefix };

export interface AgentDirectoryEntry {
  /** The canonical key, normally the identity. */
  id: string;
  /** The display name shown to people. */
  label: string;
  /** Optional role words ("Coordinator", "Memory steward"). */
  role?: string;
  /** Other keys naming the same agent: member, agent and session ids. */
  aliases?: readonly (string | null | undefined)[];
  /** The mob the agent is a member of, when its row names it. */
  mobId?: string | null;
}

export interface AgentDirectory {
  lookup(key: string | null | undefined): AgentDirectoryEntry | undefined;
  readonly size: number;
  /**
   * The host opens only the agents this directory holds, and opens the
   * agent the directory found for a key (not the key as written). Set by a
   * host whose names can belong to more than one team; absent, a linked
   * name is offered for every key and opened by the key as written.
   */
  readonly opensKnownOnly?: boolean;
  /**
   * The words a host shows in place of a key that names no agent (a name
   * the host was not told the team of): returned for such a key, undefined
   * for any other. They are written as they are, not as an agent's name,
   * and are offered no opening.
   */
  plainWords?(key: string): string | undefined;
  /**
   * The mobs, other than the console's own, with a member in the directory:
   * a key outside the directory that names a member of one
   * (`<mob id>/<member name>`) is shown by its member name.
   */
  readonly nestedMobIds?: ReadonlySet<string>;
}

export function createAgentDirectory(entries: readonly AgentDirectoryEntry[]): AgentDirectory {
  const byKey = new Map<string, AgentDirectoryEntry>();
  const nestedMobIds = new Set<string>();
  const add = (key: string | null | undefined, entry: AgentDirectoryEntry) => {
    const trimmed = typeof key === "string" ? key.trim() : "";
    if (!trimmed) return;
    if (!byKey.has(trimmed)) byKey.set(trimmed, entry);
    const bare = stripAgentKeyPrefix(trimmed);
    if (bare && !byKey.has(bare)) byKey.set(bare, entry);
  };
  for (const entry of entries) {
    if (!entry || !entry.label?.trim()) continue;
    add(entry.id, entry);
    const mobId = entry.mobId?.trim();
    if (mobId && parseNestedMemberIdentity(entry.id.trim())?.mobId === mobId) nestedMobIds.add(mobId);
  }
  // Aliases second, so a canonical id always wins over another agent's alias.
  for (const entry of entries) {
    if (!entry || !entry.label?.trim()) continue;
    for (const alias of entry.aliases ?? []) add(alias, entry);
  }
  return {
    size: entries.length,
    nestedMobIds,
    lookup(key) {
      if (typeof key !== "string") return undefined;
      const trimmed = key.trim();
      if (!trimmed) return undefined;
      return byKey.get(trimmed) ?? byKey.get(stripAgentKeyPrefix(trimmed));
    },
  };
}

export const EMPTY_AGENT_DIRECTORY: AgentDirectory = createAgentDirectory([]);

/**
 * Whether an opening of the agent a key names is offered. A host's own
 * opening may read a key more loosely than its directory (another case, a
 * label), so by default every key is offered, as it always was. A
 * directory that says its openings are for the agents it holds only
 * (`opensKnownOnly`: a host whose names can belong to more than one team)
 * offers none for a key it does not hold: such a key names nobody to open.
 */
export function agentOpenable(key: string | null | undefined, directory: AgentDirectory = EMPTY_AGENT_DIRECTORY): boolean {
  if (typeof key !== "string" || !key.trim()) return false;
  // Words in place of a key name nobody to open, whatever else is offered.
  if (directory.plainWords?.(key) !== undefined) return false;
  return directory.opensKnownOnly !== true || directory.lookup(key) !== undefined;
}

/** The display name for a key: the directory label, else the name
 * agentDisplayTitle gives the key ("identity:support-triage" reads "Support
 * triage"; a member of a nested mob the directory knows reads by its member
 * name, "research-team/reviewer" as "Reviewer"), else the empty string. */
export function agentDisplayName(
  key: string | null | undefined,
  directory: AgentDirectory = EMPTY_AGENT_DIRECTORY,
): string {
  if (typeof key !== "string" || !key.trim()) return "";
  // Words in place of a key are written as they are: read as a name they
  // would look like an agent of that name.
  const words = directory.plainWords?.(key);
  if (words !== undefined) return words;
  const entry = directory.lookup(key);
  if (entry) return entry.label.trim();
  return identityDisplayTitle(key, directory.nestedMobIds);
}

/** One letter or digit for an avatar; "?" when the name has none. */
export function agentInitial(label: string | null | undefined): string {
  const match = (label ?? "").match(/[\p{L}\p{N}]/u);
  return match ? match[0].toUpperCase() : "?";
}
