// How the console names an agent for people. Pure, with no @console-core
// imports, so every bundle that includes lib/adapters.ts can use it.

import type { ConsoleAgent } from "../types";

/** What an agent is, for people: its role, else its group, capitalized ("Reviewer"); empty when it has neither. */
export function agentKindLabel(agent: Pick<ConsoleAgent, "role" | "group">): string {
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
 * one. Without one (its label is just one of its ids), a readable identity
 * reads as words ("router:main" reads "Router main"), and a generated id
 * reads as the agent's kind and the id's last six characters ("Reviewer
 * 4e5f60"). The raw identity belongs in a tooltip or a copy action, not in
 * the name.
 */
export function agentDisplayTitle(
  agent: Pick<ConsoleAgent, "label" | "identity" | "member_id" | "agent_id" | "labels" | "role" | "group">,
): string {
  const label = (agent.label || "").trim();
  const ids = [agent.identity, agent.member_id, agent.agent_id, agent.labels?.agent_identity]
    .map((value) => (typeof value === "string" ? value.trim() : ""))
    .filter(Boolean);
  if (label && !ids.includes(label)) return label;
  const id = label || ids[0] || "";
  const parts = id.split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  if (parts.length === 0) return id || "Agent";
  if (parts.some(isOpaqueIdPart)) {
    return `${agentKindLabel(agent) || "Agent"} ${parts.join("").slice(-6).toLowerCase()}`;
  }
  const words = parts.join(" ");
  return words[0]!.toUpperCase() + words.slice(1);
}
