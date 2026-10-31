// The topology's relation inputs, from the gateway's own reads: the routes
// that feed agents from channels (meta/routing/routes/list, with the
// delivery history matched onto them), the typed lineage of helpers, forks
// and side chats (meta/mob_events/query, member_spawned), and the agents'
// own memory scopes (meta/memory/panel/overview). Pure functions: responses
// in, the component package's relation inputs out.

import type {
  ConsoleTopologyAgentStatus,
  ConsoleTopologyGrouping,
  ConsoleTopologyLineage,
  ConsoleTopologyLineageKind,
  ConsoleTopologyMemoryScope,
  ConsoleTopologyRoute,
  ConsoleTopologyRoutesAccess,
  ConsoleTopologyStore,
} from "../../../packages/console-components/src/topology/types";
import type { ConsoleAgent, ConsoleAgentListConfig, ConsoleFrame } from "../types";
import { AGENT_BUCKET_ORDER, bucketOf } from "./agent-buckets";
import { agentHealth, isAgentWorking } from "./agent-status";
import { CONSOLE_COMMAND_NAMES, type ConsoleCommandName } from "./headless";

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

/** The team's shared memory store (the SHARED lane), with the records every scope holds. */
export function normalizeTopologyMemoryStore(overview: unknown): ConsoleTopologyStore {
  let records = 0;
  for (const raw of list(overview, "scopes")) {
    if (isRecord(raw)) records += finite(raw.active) ?? 0;
  }
  return { id: "team", label: "Team memory", records };
}

function configuredValue(agent: ConsoleAgent, selector: string): string | null {
  const name = selector.trim();
  if (!name) return null;
  if (name.startsWith("labels.")) return text(agent.labels?.[name.slice("labels.".length)]) || null;
  if (name.startsWith("label:")) return text(agent.labels?.[name.slice("label:".length)]) || null;
  switch (name) {
    case "group": return text(agent.group) || null;
    case "subgroup": return text(agent.subgroup) || null;
    case "role": return text(agent.role) || null;
    case "kind": return text(agent.kind) || null;
    case "identity": return text(agent.identity) || null;
    case "member_id": return text(agent.member_id) || null;
    case "agent_id": return text(agent.agent_id) || null;
    default: return text(agent.labels?.[name]) || null;
  }
}

/**
 * The map's lanes in the sidebar's words: the configured grouping
 * (`agent_list.group_by`, then its fallback group) when the console names
 * one, else the sidebar's buckets; the configured section order first.
 */
export function topologyGrouping(config: ConsoleAgentListConfig | null | undefined): ConsoleTopologyGrouping {
  if (!config) return BUCKET_GROUPING;
  const cached = groupingByConfig.get(config);
  if (cached) return cached;
  const grouping = buildTopologyGrouping(config);
  groupingByConfig.set(config, grouping);
  return grouping;
}

// One grouping per configuration object, so a render passes the same rule
// and the map does not rebuild its lanes.
const groupingByConfig = new WeakMap<ConsoleAgentListConfig, ConsoleTopologyGrouping>();

function buildTopologyGrouping(config: ConsoleAgentListConfig | null | undefined): ConsoleTopologyGrouping {
  const selectors = (config?.group_by || []).map((value) => value.trim()).filter(Boolean);
  const configuredOrder = (config?.section_order || []).map((value) => value.trim()).filter(Boolean);
  if (selectors.length > 0) {
    const fallback = text(config?.fallback_group) || "Agents";
    return {
      sectionOf: (agent) => {
        for (const selector of selectors) {
          const value = configuredValue(agent, selector);
          if (value) return value;
        }
        return fallback;
      },
      order: configuredOrder,
    };
  }
  return {
    sectionOf: (agent) => bucketOf(agent),
    order: configuredOrder.length > 0 ? [...configuredOrder, ...AGENT_BUCKET_ORDER] : AGENT_BUCKET_ORDER,
  };
}

/**
 * The console's one status rule for an agent on the map: working now (the
 * owner's phase, an open run, or a run the console saw start), and health
 * that needs attention as the node's badge. A busy agent reads as working,
 * never as unobserved.
 */
export function topologyAgentStatus(agent: ConsoleAgent, openRun = false): ConsoleTopologyAgentStatus {
  const working = isAgentWorking(agent, { openRun });
  const health = agentHealth(agent);
  const retired = health.kind === "retired" || health.kind === "retiring";
  return {
    working,
    health: health.problem ? (health.tone === "crit" ? "crit" : "warn") : null,
    healthWord: health.problem ? health.word : null,
    retired,
  };
}

const BUCKET_GROUPING: ConsoleTopologyGrouping = buildTopologyGrouping(null);

/** What the topology read beside the query, and when. */
export interface ConsoleTopologyRelationsState {
  routes: ConsoleTopologyRoute[] | null;
  routesAccess: ConsoleTopologyRoutesAccess | null;
  lineage: ConsoleTopologyLineage[] | null;
  memoryScopes: ConsoleTopologyMemoryScope[] | null;
  stores: ConsoleTopologyStore[] | null;
  /** When the last refresh answered. */
  updatedAtMs: number | null;
  /** Why the last refresh failed; the map keeps the data it had. */
  error: string | null;
}

export const EMPTY_TOPOLOGY_RELATIONS: ConsoleTopologyRelationsState = {
  routes: null,
  routesAccess: null,
  lineage: null,
  memoryScopes: null,
  stores: null,
  updatedAtMs: null,
  error: null,
};

export type TopologyRelationTarget = "topology" | "routing" | "memory";

export interface TopologyRelationReads {
  /** The methods the runtime advertises. */
  methods: readonly string[];
  /** A command's RPC method. */
  methodOf: (command: ConsoleCommandName) => string;
  execute: (command: ConsoleCommandName, target: TopologyRelationTarget, params?: Record<string, unknown>) => Promise<unknown>;
  /** Memory is set up and readable (the SHARED lane and the memory arcs). */
  memoryAvailable: boolean;
}

/**
 * The relations the map draws beyond the query: routes (when the caller may
 * read routing), typed lineage (mob events) and the agents' memory scopes.
 * Each read is optional and never fails the whole: one that is not
 * advertised or fails leaves only its own relation out, and a refused routes
 * read says why.
 */
export async function readTopologyRelations(
  reads: TopologyRelationReads,
): Promise<Omit<ConsoleTopologyRelationsState, "updatedAtMs" | "error">> {
  const has = (command: ConsoleCommandName) => reads.methods.includes(reads.methodOf(command));
  const routesRead: Promise<Pick<ConsoleTopologyRelationsState, "routes" | "routesAccess">> = has(CONSOLE_COMMAND_NAMES.listRoutingRoutes)
    ? Promise.all([
        reads.execute(CONSOLE_COMMAND_NAMES.listRoutingRoutes, "routing"),
        has(CONSOLE_COMMAND_NAMES.listDeliveryHistory)
          ? reads.execute(CONSOLE_COMMAND_NAMES.listDeliveryHistory, "routing").catch(() => null)
          : Promise.resolve(null),
      ]).then(
        ([routes, history]) => ({ routes: normalizeTopologyRoutes(routes, history), routesAccess: "loaded" as const }),
        (error: unknown) => ({ routes: null, routesAccess: topologyRoutesAccessFromError(error) }),
      )
    : Promise.resolve({ routes: null, routesAccess: "unavailable" as const });
  const lineageRead = has(CONSOLE_COMMAND_NAMES.topologyMobEvents)
    ? reads.execute(CONSOLE_COMMAND_NAMES.topologyMobEvents, "topology", {
        event_types: [...TOPOLOGY_LINEAGE_EVENT_TYPES],
        limit: TOPOLOGY_LINEAGE_EVENT_LIMIT,
      }).then(normalizeTopologyLineage, () => null)
    : Promise.resolve(null);
  const memoryRead = reads.memoryAvailable && has(CONSOLE_COMMAND_NAMES.getMemoryOverview)
    ? reads.execute(CONSOLE_COMMAND_NAMES.getMemoryOverview, "memory").then(
        (overview) => ({ memoryScopes: normalizeTopologyMemoryScopes(overview), stores: [normalizeTopologyMemoryStore(overview)] }),
        () => null,
      )
    : Promise.resolve(null);
  const [routes, lineage, memory] = await Promise.all([routesRead, lineageRead, memoryRead]);
  return {
    ...routes,
    lineage,
    memoryScopes: memory?.memoryScopes ?? null,
    stores: memory?.stores ?? null,
  };
}

/** One page of the recent timeline, newest first. */
export interface TopologyTimelinePage {
  frames: readonly ConsoleFrame[];
  nextCursor?: string;
  exhausted?: boolean;
}

/** How many pages one backfill reads at most (1000 frames each). */
export const TOPOLOGY_BACKFILL_MAX_PAGES = 10;
/** How far back a backfill reads: the map's longest window. */
export const TOPOLOGY_BACKFILL_WINDOW_MS = 24 * 60 * 60 * 1000;

/**
 * Reads the recent timeline back to `sinceMs` (or `maxPages` pages) and hands
 * it to the ledger in one batch. A message counts once only when the ledger
 * sees its ends together (the sender's call with its receipt, the
 * recipient's notice), so a backfill of the newest page alone splits the
 * messages at that page's edge and counts them twice. Returns how many
 * frames it read.
 */
export async function backfillTopologyLedger(options: {
  query: (before: string | undefined) => Promise<TopologyTimelinePage>;
  ingest: (frames: readonly ConsoleFrame[]) => void;
  sinceMs: number;
  maxPages?: number;
}): Promise<number> {
  const maxPages = options.maxPages ?? TOPOLOGY_BACKFILL_MAX_PAGES;
  const frames: ConsoleFrame[] = [];
  let before: string | undefined;
  for (let page = 0; page < maxPages; page += 1) {
    const result = await options.query(before);
    frames.push(...result.frames);
    const oldest = result.frames.reduce(
      (min, frame) => (typeof frame.timestampMs === "number" ? Math.min(min, frame.timestampMs) : min),
      Number.POSITIVE_INFINITY,
    );
    if (result.frames.length === 0 || result.exhausted || !result.nextCursor || oldest < options.sinceMs) break;
    before = result.nextCursor;
  }
  if (frames.length > 0) options.ingest(frames);
  return frames.length;
}
