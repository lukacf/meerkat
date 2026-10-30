// Agent names for panels. Panels receive identity, member and session keys
// from the runtime; people read display names. A directory maps every key
// an agent is known by to its label. Pure module (no React), so panels whose
// test bundles cannot resolve React can still resolve names.

export interface AgentDirectoryEntry {
  /** The canonical key, normally the identity. */
  id: string;
  /** The display name shown to people. */
  label: string;
  /** Optional role words ("Coordinator", "Memory steward"). */
  role?: string;
  /** Other keys naming the same agent: member, agent and session ids. */
  aliases?: readonly (string | null | undefined)[];
}

export interface AgentDirectory {
  lookup(key: string | null | undefined): AgentDirectoryEntry | undefined;
  readonly size: number;
}

const KEY_PREFIXES = ["identity:", "agent:", "member:"];

/** A key without its kind prefix: "identity:support-triage" reads
 * "support-triage". Capitals are never invented. */
export function stripAgentKeyPrefix(key: string): string {
  let current = key.trim();
  let changed = true;
  while (changed) {
    changed = false;
    for (const prefix of KEY_PREFIXES) {
      if (current.toLowerCase().startsWith(prefix) && current.length > prefix.length) {
        current = current.slice(prefix.length);
        changed = true;
      }
    }
  }
  return current;
}

export function createAgentDirectory(entries: readonly AgentDirectoryEntry[]): AgentDirectory {
  const byKey = new Map<string, AgentDirectoryEntry>();
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
  }
  // Aliases second, so a canonical id always wins over another agent's alias.
  for (const entry of entries) {
    if (!entry || !entry.label?.trim()) continue;
    for (const alias of entry.aliases ?? []) add(alias, entry);
  }
  return {
    size: entries.length,
    lookup(key) {
      if (typeof key !== "string") return undefined;
      const trimmed = key.trim();
      if (!trimmed) return undefined;
      return byKey.get(trimmed) ?? byKey.get(stripAgentKeyPrefix(trimmed));
    },
  };
}

export const EMPTY_AGENT_DIRECTORY: AgentDirectory = createAgentDirectory([]);

/** The display name for a key: the directory label, else the key without its
 * kind prefix, else the empty string. */
export function agentDisplayName(
  key: string | null | undefined,
  directory: AgentDirectory = EMPTY_AGENT_DIRECTORY,
): string {
  if (typeof key !== "string" || !key.trim()) return "";
  const entry = directory.lookup(key);
  if (entry) return entry.label.trim();
  return stripAgentKeyPrefix(key);
}

/** One letter or digit for an avatar; "?" when the name has none. */
export function agentInitial(label: string | null | undefined): string {
  const match = (label ?? "").match(/[\p{L}\p{N}]/u);
  return match ? match[0].toUpperCase() : "?";
}
