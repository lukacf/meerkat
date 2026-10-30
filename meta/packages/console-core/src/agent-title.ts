// How the console names an agent for people: one function for every
// surface (the stock console, the kit's name fallback and the shared-host
// builders), so one agent has one name everywhere. Pure, with no runtime
// imports, so every test bundle can include it by a relative path.

/** The fields an agent name is built from. */
export interface AgentTitleSource {
  label?: string | null;
  identity?: string | null;
  member_id?: string | null;
  agent_id?: string | null;
  labels?: Readonly<Record<string, string | undefined>> | null;
  role?: string | null;
  group?: string | null;
}

const KEY_PREFIXES = ["identity:", "agent:", "member:"];

/** A key without its kind prefix: "identity:support-triage" reads
 * "support-triage". */
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

/** What an agent is, for people: its role, else its group, capitalized ("Reviewer"); empty when it has neither. */
export function agentKindLabel(agent: Pick<AgentTitleSource, "role" | "group">): string {
  const kind = (agent.role || agent.group || "").trim().replace(/[_-]+/g, " ").replace(/\s+/g, " ");
  return kind ? kind[0]!.toUpperCase() + kind.slice(1) : "";
}

/**
 * Whether a piece of an identity reads as a generated id rather than a word:
 * eight or more characters with a digit, other than a word with a short
 * number ("worker2024").
 */
function isOpaqueIdPart(part: string): boolean {
  return part.length >= 8 && /\d/.test(part) && !/^\p{L}+\d{1,4}$/u.test(part);
}

/**
 * The name to show for an agent, for people. Its display name when it has
 * one. Without one (its label is just one of its ids), the id loses its kind
 * prefix (identity:, agent:, member:) and a readable rest reads as words
 * ("identity:queue-agent" reads "Queue agent", "router:main" reads "Router
 * main"), while a generated id reads as the agent's kind and the id's last
 * six characters ("Reviewer 4e5f60"). The raw identity belongs in a tooltip
 * or a copy action, not in the name.
 */
export function agentDisplayTitle(agent: AgentTitleSource): string {
  const label = (agent.label || "").trim();
  const ids = [agent.identity, agent.member_id, agent.agent_id, agent.labels?.agent_identity]
    .map((value) => (typeof value === "string" ? value.trim() : ""))
    .filter(Boolean);
  if (label && !ids.includes(label)) return label;
  const id = label || ids[0] || "";
  const bare = stripAgentKeyPrefix(id);
  // An email already reads as a person's address; it is not an agent key.
  if (bare.includes("@")) return bare;
  const parts = bare.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (parts.length === 0) return id || "Agent";
  if (parts.some(isOpaqueIdPart)) {
    return `${agentKindLabel(agent) || "Agent"} ${parts.join("").slice(-6).toLowerCase()}`;
  }
  const words = parts.join(" ");
  return words[0]!.toUpperCase() + words.slice(1);
}
