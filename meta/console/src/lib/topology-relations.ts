// The topology's relation inputs, from the gateway's own reads: the routes
// that feed agents from channels (meta/routing/routes/list, with the
// delivery history matched onto them), the typed lineage of helpers, forks
// and side chats (meta/mob_events/query, member_spawned), and the agents'
// own memory scopes (meta/memory/panel/overview). Pure functions: responses
// in, the component package's relation inputs out.

import type {
  ConsoleTopologyLineage,
  ConsoleTopologyLineageKind,
  ConsoleTopologyMemoryScope,
  ConsoleTopologyRoute,
  ConsoleTopologyRoutesAccess,
} from "../../../packages/console-components/src/topology/types";

/** The route keys the runtime keeps for every member on its own (roster routes). */
const ROSTER_ROUTE_PREFIX = "mob.member.";
const ROSTER_ROUTE_SINK = "mob_member";
/** The structural mob event that names who started a member. */
export const TOPOLOGY_LINEAGE_EVENT_TYPES: readonly string[] = ["member_spawned"];
/** How many lineage events one read asks for. */
export const TOPOLOGY_LINEAGE_EVENT_LIMIT = 2000;

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function text(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function finite(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function list(value: unknown, key: string): unknown[] {
  if (Array.isArray(value)) return value;
  return isRecord(value) && Array.isArray(value[key]) ? value[key] as unknown[] : [];
}

/**
 * Routes from `meta/routing/routes/list`, with the deliveries of
 * `meta/delivery/history` counted onto the route they match. Deliveries name
 * a resolution id (`route-000123`), not the route key, so they match on the
 * route's recipient, sink and module; a delivery two routes share counts on
 * each (the server change S1 in the design spec would make this exact).
 */
export function normalizeTopologyRoutes(
  routesResponse: unknown,
  historyResponse?: unknown,
): ConsoleTopologyRoute[] {
  const deliveries = list(historyResponse, "deliveries").filter(isRecord);
  const routes: ConsoleTopologyRoute[] = [];
  for (const raw of list(routesResponse, "routes")) {
    if (!isRecord(raw)) continue;
    const key = text(raw.route_key);
    const recipient = text(raw.recipient);
    if (!key || !recipient) continue;
    const channel = text(raw.channel) || "default";
    const sink = text(raw.sink) || null;
    const module = text(raw.target_module) || null;
    let sent = 0;
    let failed = 0;
    let lastAtMs: number | null = null;
    for (const delivery of deliveries) {
      if (text(delivery.recipient) !== recipient) continue;
      if (sink && text(delivery.sink) && text(delivery.sink) !== sink) continue;
      if (module && text(delivery.target_module) && text(delivery.target_module) !== module) continue;
      const status = text(delivery.status);
      if (status === "sent" || status === "delivered") sent += 1;
      else if (status === "failed" || status === "dead_letter" || status === "delivery_failed") failed += 1;
      const at = finite(delivery.final_attempt_ms) ?? finite(delivery.first_attempt_ms);
      if (at !== null && (lastAtMs === null || at > lastAtMs)) lastAtMs = at;
    }
    routes.push({
      key,
      channel,
      recipient,
      sink,
      module,
      managed: key.startsWith(ROSTER_ROUTE_PREFIX) && (!sink || sink === ROSTER_ROUTE_SINK),
      deliveries: sent || failed ? { sent, failed, lastAtMs } : null,
    });
  }
  return routes;
}

/**
 * Who started each member, from `member_spawned` events: a fork names its
 * source member (`fork_source.source_member.member`), a spawn its spawner
 * (`spawned_by`), and a creation provenance its source creation, which an
 * earlier spawn event names. A fork a person opened from an agent's
 * conversation (a side chat, `labels.side_chat_of`) reads as a side chat. The
 * newest event that names a parent wins; a respawn that names none keeps it.
 */
export function normalizeTopologyLineage(mobEventsResponse: unknown): ConsoleTopologyLineage[] {
  const events = list(mobEventsResponse, "events").filter(isRecord);
  const identityByCreation = new Map<string, string>();
  for (const event of events) {
    const data = isRecord(event.data) ? event.data : {};
    if (text(data.type) !== "member_spawned" && text(event.kind) !== "member_spawned") continue;
    const identity = text(data.agent_identity) || text(event.agent_identity);
    const creation = isRecord(data.creation) ? text(data.creation.creation_id) : "";
    if (identity && creation) identityByCreation.set(creation, identity);
  }
  const byChild = new Map<string, ConsoleTopologyLineage>();
  for (const event of events) {
    const data = isRecord(event.data) ? event.data : {};
    if (text(data.type) !== "member_spawned" && text(event.kind) !== "member_spawned") continue;
    const child = text(data.agent_identity) || text(event.agent_identity);
    if (!child) continue;
    const labels = isRecord(data.labels) ? data.labels : {};
    const forkSource = isRecord(data.fork_source) && isRecord(data.fork_source.source_member)
      ? text(data.fork_source.source_member.member)
      : "";
    const provenance = isRecord(data.creation) && isRecord(data.creation.provenance) ? data.creation.provenance : {};
    const fromProvenance = text(provenance.kind) === "fork"
      ? identityByCreation.get(text(provenance.source_creation_id)) || ""
      : "";
    const spawnedBy = text(data.spawned_by) || text(labels.spawned_by);
    const sideChatOf = text(labels.side_chat_of);
    let parent = "";
    let kind: ConsoleTopologyLineageKind = "spawn";
    if (sideChatOf) {
      parent = sideChatOf;
      kind = "side_chat";
    } else if (forkSource || fromProvenance) {
      parent = forkSource || fromProvenance;
      kind = "fork";
    } else if (text(labels.delegate_host_identity)) {
      parent = text(labels.delegate_host_identity);
      kind = "delegate";
    } else if (spawnedBy) {
      parent = spawnedBy;
    }
    // A later spawn of the same member that names no parent (a respawn)
    // keeps the lineage an earlier one named.
    if (!parent || parent === child) continue;
    byChild.set(child, { parent, child, kind, atMs: finite(event.timestamp_ms) });
  }
  return Array.from(byChild.values());
}

/** The agents' own memory: identity scopes of the memory overview, with their active records. */
export function normalizeTopologyMemoryScopes(overview: unknown): ConsoleTopologyMemoryScope[] {
  const scopes: ConsoleTopologyMemoryScope[] = [];
  for (const raw of list(overview, "scopes")) {
    if (!isRecord(raw) || text(raw.scope_kind) !== "identity") continue;
    const identity = text(raw.scope_key);
    if (!identity) continue;
    scopes.push({
      identity,
      records: finite(raw.active) ?? 0,
      quarantined: finite(raw.quarantined) ?? 0,
    });
  }
  return scopes;
}

/**
 * Why a routes read failed: `denied` when the caller lacks routing access
 * (the access refusal, -32030 or `data.kind` access_denied, or the older
 * -32001), `unavailable` for anything else.
 */
export function topologyRoutesAccessFromError(error: unknown): ConsoleTopologyRoutesAccess {
  const rpcError = isRecord(error) && isRecord(error.rpcError) ? error.rpcError : null;
  const code = finite(rpcError?.code);
  const kind = rpcError && isRecord(rpcError.data) ? text(rpcError.data.kind) : "";
  if (kind === "access_denied" || code === -32030 || code === -32001) return "denied";
  return "unavailable";
}
