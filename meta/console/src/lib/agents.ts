import {
  normalizeIdentityStatusRow,
  normalizeResponsePhase,
  normalizeSidebarWatchFields,
  normalizeUnobservedFacts,
} from "@console-core";
import type { ConsoleAgent, ConsoleExperience, ConsoleExperienceAgentSnapshotRow } from "../types";
import { agentDisplayTitle } from "./agent-title";

/// Alias to canonical identity for one roster. Built once per roster change
/// and consulted per frame; the per-frame path used to rebuild a four-entry
/// alias array for every agent (160 agents x 20 frames/s).
export type ConsoleIdentityAliasMap = Map<string, string>;

export function buildConsoleIdentityAliasMap(agents: ConsoleAgent[]): ConsoleIdentityAliasMap {
  const map: ConsoleIdentityAliasMap = new Map();
  for (const agent of agents) {
    const canonical = (agent.identity || agent.member_id || agent.agent_id || "").trim();
    if (!canonical) continue;
    const labelIdentity =
      typeof agent.labels?.agent_identity === "string"
        ? agent.labels.agent_identity.trim()
        : "";
    for (const alias of [agent.identity, agent.member_id, agent.agent_id, labelIdentity]) {
      const key = alias?.trim();
      // First roster entry wins, matching the linear scan's precedence.
      if (key && !map.has(key)) map.set(key, canonical);
    }
  }
  return map;
}

export function canonicalConsoleIdentityFromMap(
  identity: string | undefined,
  aliases: ConsoleIdentityAliasMap,
): string {
  const normalized = identity?.trim() || "";
  if (!normalized) return "";
  return aliases.get(normalized) ?? normalized;
}

export function canonicalConsoleIdentity(
  identity: string | undefined,
  agents: ConsoleAgent[],
): string {
  const normalized = identity?.trim() || "";
  if (!normalized) return "";
  for (const agent of agents) {
    const labelIdentity =
      typeof agent.labels?.agent_identity === "string"
        ? agent.labels.agent_identity.trim()
        : "";
    const aliases = [
      agent.identity,
      agent.member_id,
      agent.agent_id,
      labelIdentity,
    ]
      .filter((value): value is string => Boolean(value?.trim()))
      .map((value) => value.trim());
    if (!aliases.includes(normalized)) continue;
    return (agent.identity || agent.member_id || agent.agent_id || normalized).trim();
  }
  return normalized;
}

function normalizeModelCapabilities(entry: unknown): { image_input: boolean } {
  const record = entry && typeof entry === "object" ? entry as Record<string, unknown> : {};
  const caps = record.model_capabilities && typeof record.model_capabilities === "object"
    ? record.model_capabilities as Record<string, unknown>
    : {};
  return { image_input: caps.image_input === true };
}

/** A roster row's advisory last activity time; anything but a non-negative finite number reads as unknown. */
export function normalizeLastActivityMs(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : undefined;
}

// The naming helpers live in a module without @console-core imports, so
// lib/adapters.ts (bundled without that alias in some test lanes) can use them.
export { agentDisplayTitle, agentKindLabel } from "./agent-title";

/**
 * An agent's entry in the panels' name directory, named the way the sidebar
 * names it ({@link agentDisplayTitle}), so a panel and the sidebar never
 * call the same agent two different things.
 */
export function agentDirectoryEntry(
  agent: Pick<ConsoleAgent, "label" | "identity" | "member_id" | "agent_id" | "session_id" | "labels" | "role" | "group">,
): { id: string; label: string; role?: string; aliases: (string | undefined)[] } {
  return {
    id: agent.identity || agent.agent_id,
    label: agentDisplayTitle(agent),
    role: agent.role,
    aliases: [agent.member_id, agent.agent_id, agent.session_id],
  };
}

/** The first well-formed last activity time among a row's sources, as an agent field. */
function lastActivityField(...values: unknown[]): Pick<ConsoleAgent, "last_activity_ms"> {
  for (const value of values) {
    const at = normalizeLastActivityMs(value);
    if (at !== undefined) return { last_activity_ms: at };
  }
  return {};
}

export function normalizeAgents(
  experience: ConsoleExperience | null,
  modules: unknown[]
): ConsoleAgent[] {
  const identityStatusRows = Array.isArray(experience?.identity_status?.rows)
    ? experience.identity_status.rows
    : [];
  const normalizedIdentityStatusRows = identityStatusRows
    .map((entry) => normalizeIdentityStatusRow(entry))
    .filter((entry): entry is NonNullable<typeof entry> => entry !== null);
  const identityStatusByIdentity = new Map(
    normalizedIdentityStatusRows.map((row) => [row.identity, row] as const),
  );

  const snapshotAgents = experience?.agent_sidebar?.live_snapshot?.agents;
  if (Array.isArray(snapshotAgents) && snapshotAgents.length > 0) {
    const agents: ConsoleAgent[] = snapshotAgents.map((entry: ConsoleExperienceAgentSnapshotRow) => {
      const entryIdentity = typeof entry.identity === "string" ? entry.identity.trim() : "";
      const entryMemberId = typeof entry.member_id === "string" ? entry.member_id.trim() : "";
      const entryLabels =
        entry.labels && typeof entry.labels === "object"
          ? entry.labels as Record<string, unknown>
          : {};
      const durableAgentIdentity =
        typeof entryLabels.agent_identity === "string"
          ? entryLabels.agent_identity.trim()
          : "";
      const statusRow =
        identityStatusByIdentity.get(durableAgentIdentity) ||
        identityStatusByIdentity.get(entryIdentity) ||
        identityStatusByIdentity.get(entryMemberId) ||
        normalizeIdentityStatusRow(entry);
      const watchFields = normalizeSidebarWatchFields(entry);
      const responsePhase = normalizeResponsePhase(entry.response_phase);
      const modelCapabilities = entry.model_capabilities !== undefined
        ? normalizeModelCapabilities(entry)
        : normalizeModelCapabilities(identityStatusRows.find((row) => {
          const normalized = normalizeIdentityStatusRow(row);
          return normalized?.identity === statusRow?.identity;
        }));

      return {
        ...(statusRow?.identity
          ? { identity: statusRow.identity }
          : durableAgentIdentity
            ? { identity: durableAgentIdentity }
            : entry.identity
              ? { identity: String(entry.identity) }
              : {}),
        agent_id: String(entry.agent_id || statusRow?.identity || durableAgentIdentity || entry.identity || entry.member_id || ""),
        member_id: String(entry.member_id || statusRow?.identity || entry.identity || entry.agent_id || ""),
        ...(typeof entry.session_id === "string" && entry.session_id.trim() ? { session_id: entry.session_id.trim() } : {}),
        label: String(entry.label || statusRow?.display_name || entry.display_name || statusRow?.identity || entry.identity || entry.member_id || entry.agent_id || "unknown"),
        kind: String(entry.kind || statusRow?.role || entry.role || "module_agent"),
        ...(statusRow?.role !== undefined
          ? { role: statusRow.role }
          : entry.role !== undefined
            ? { role: String(entry.role) }
            : {}),
        ...(statusRow?.state !== undefined
          ? { state: statusRow.state }
          : entry.state !== undefined
            ? { state: String(entry.state) }
            : {}),
        ...(statusRow?.addressability ? { addressability: statusRow.addressability } : {}),
        ...(statusRow?.generation !== undefined ? { generation: statusRow.generation } : {}),
        ...(statusRow?.progress !== undefined ? { progress: statusRow.progress } : {}),
        ...((entry.response_phase === null || responsePhase !== null) && { response_phase: responsePhase }),
        ...(entry.wired_to !== undefined && { wired_to: entry.wired_to as string[] }),
        ...(statusRow?.labels && Object.keys(statusRow.labels).length > 0
          ? { labels: statusRow.labels }
          : entry.labels !== undefined
            ? { labels: entry.labels as Record<string, string> }
            : {}),
        ...(entry.group !== undefined && { group: String(entry.group) }),
        ...(entry.subgroup !== undefined && { subgroup: String(entry.subgroup) }),
        ...(entry.addressable !== undefined
          ? { addressable: Boolean(entry.addressable) }
          : statusRow?.addressability
            ? { addressable: statusRow.addressability === "addressable" }
            : {}),
        ...(entry.affordances !== undefined && { affordances: entry.affordances }),
        ...(typeof entry.side_chat_of === "string" && entry.side_chat_of.trim()
          ? { side_chat_of: entry.side_chat_of.trim() }
          : {}),
        ...lastActivityField(entry.last_activity_ms, statusRow?.last_activity_ms),
        model_capabilities: modelCapabilities,
        ...watchFields,
        ...((): Pick<ConsoleAgent, "unobserved"> => {
          const unobserved = normalizeUnobservedFacts(entry.unobserved) ?? statusRow?.unobserved;
          return unobserved ? { unobserved } : {};
        })(),
      };
    });
    const seen = new Set(
      agents.flatMap((agent) => [agent.identity, agent.member_id, agent.agent_id])
        .filter((value): value is string => Boolean(value))
        .map((value) => value.toLowerCase()),
    );
    for (const statusRow of normalizedIdentityStatusRows) {
      if (seen.has(statusRow.identity.toLowerCase())) continue;
      const addressable = statusRow.addressability === "addressable";
      agents.push({
        identity: statusRow.identity,
        agent_id: statusRow.identity,
        member_id: statusRow.identity,
        label: String(statusRow.display_name || statusRow.identity),
        kind: String(statusRow.role || "identity"),
        ...(statusRow.role !== undefined ? { role: statusRow.role } : {}),
        state: statusRow.state,
        addressability: statusRow.addressability,
        ...(statusRow.generation !== undefined ? { generation: statusRow.generation } : {}),
        ...(statusRow.progress !== undefined ? { progress: statusRow.progress } : {}),
        ...(statusRow.labels && Object.keys(statusRow.labels).length > 0 ? { labels: statusRow.labels } : {}),
        ...(statusRow.labels?.group ? { group: statusRow.labels.group } : {}),
        ...(statusRow.labels?.console_subgroup
          ? { subgroup: statusRow.labels.console_subgroup }
          : statusRow.labels?.org
            ? { subgroup: statusRow.labels.org }
            : {}),
        addressable,
        affordances: { can_send_message: addressable },
        model_capabilities: { image_input: false },
        ...lastActivityField(statusRow.last_activity_ms),
        ...(statusRow.unobserved ? { unobserved: statusRow.unobserved } : {}),
      });
      seen.add(statusRow.identity.toLowerCase());
    }
    return agents;
  }

  if (Array.isArray(identityStatusRows) && identityStatusRows.length > 0) {
    return identityStatusRows.map((entry) => {
      const statusRow = normalizeIdentityStatusRow(entry);
      const identity = statusRow?.identity || "";
      const modelCapabilities = normalizeModelCapabilities(entry);

      return {
        identity,
        agent_id: String(identity),
        member_id: identity ? `identity-only:${identity}` : "",
        ...(typeof statusRow?.session_id === "string" && statusRow.session_id.trim() ? { session_id: statusRow.session_id.trim() } : {}),
        label: String(statusRow?.display_name || identity || "unknown"),
        kind: String(statusRow?.role || "identity"),
        ...(statusRow?.role !== undefined ? { role: statusRow.role } : {}),
        ...(statusRow?.state !== undefined ? { state: statusRow.state } : {}),
        ...(statusRow?.addressability ? { addressability: statusRow.addressability } : {}),
        ...(statusRow?.generation !== undefined ? { generation: statusRow.generation } : {}),
        ...(statusRow?.progress !== undefined ? { progress: statusRow.progress } : {}),
        ...(statusRow?.labels && Object.keys(statusRow.labels).length > 0 ? { labels: statusRow.labels } : {}),
        addressable: false,
        affordances: { can_send_message: false },
        model_capabilities: modelCapabilities,
        ...lastActivityField(statusRow?.last_activity_ms),
        ...(statusRow?.unobserved ? { unobserved: statusRow.unobserved } : {}),
      };
    });
  }

  if (Array.isArray(modules) && modules.length > 0) {
    return modules.map((moduleId) => ({
      agent_id: String(moduleId),
      member_id: String(moduleId),
      label: String(moduleId),
      kind: "module_agent",
      model_capabilities: { image_input: false },
    }));
  }

  return [];
}
