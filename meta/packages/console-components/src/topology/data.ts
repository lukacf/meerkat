// The topology model: everything the console knows about a team, merged
// into one graph of agents and the relations between them.
//
// Nodes come from every source the host has (the topology query's nodes, the
// experience snapshot's nodes and the roster's agents) and merge on one
// canonical key: the agent's identity, authority-qualified only for another
// team's agent. Frames, routes and lineage name agents by identity, member
// id, a peer name ("team/role/identity") or an endpoint key; one resolver
// maps every such reference to that key, so a relation never drops because
// of an alias.
//
// Relations (design spec A3.9, B1): wired (comms trust), declared but not
// wired (a degraded wiring, or one an operator suppressed), cross (another
// team), pending (a connect or disconnect in flight), lineage (who started a
// helper, fork or side chat), route (a channel feeding an agent) and traffic
// (peer messages in a window). Traffic is read from real frames only: a
// message the console cannot attribute to both ends is not drawn.

import React from "react";
import {
  agentDisplayTitle,
  canonicalTopologyEdge,
  topologyEdgeKey,
  topologyOperationIsPending,
  type ResponsePhase,
  type TopologyEdgeRef,
  type TopologyEndpointRef,
  type TopologyMutationKind,
  type TopologyOperationReceipt,
} from "@console-core";
import type {
  ConsoleAgent,
  ConsoleFrame,
  ConsoleTopologyLineage,
  ConsoleTopologyLineageKind,
  ConsoleTopologyLink,
  ConsoleTopologyMessage,
  ConsoleTopologyNode,
  ConsoleTopologyNodePresentation,
  ConsoleTopologyRoute,
  ConsoleTopologyStore,
  ConsoleTopologyTraffic,
} from "./types";

/** What a node is on the map. */
export type TopoNodeKind = "agent" | "coordinator" | "helper" | "external" | "unknown";

/** The relations the map draws between nodes. */
export type TopoRelation = "wired" | "traffic" | "route" | "lineage" | "cross";

/** An edge's state over its relation. */
export type TopoEdgeState = "normal" | "pending" | "failed" | "declared";

export interface TopoAgent {
  /** Canonical key: the identity, or `identity@authority` for another team's agent. */
  id: string;
  /** The endpoint the topology controls name (connect and disconnect act on `ref.id`). */
  ref: TopologyEndpointRef;
  label: string;
  role: string;
  state: string;
  /** Canonical ids of the peers this node's own sources list as wired. */
  wiredTo: string[];
  group: string;
  subgroup?: string;
  labels: Record<string, string>;
  presentation?: ConsoleTopologyNodePresentation;
  memberId?: string;
  responsePhase?: ResponsePhase;
  /** The roster's agent, when the console lists one. */
  agent?: ConsoleAgent;
  kind: TopoNodeKind;
  /** Another team's (another authority's) agent. */
  external: boolean;
  authority?: string | null;
  /** A helper's parent (its lineage edge's other end). */
  parent?: string;
  lineage?: ConsoleTopologyLineageKind;
}

/** A channel that feeds agents through routes (the CHANNELS lane). */
export interface TopoSource {
  id: string;
  channel: string;
  label: string;
  routes: ConsoleTopologyRoute[];
  /** Canonical ids of the agents its routes feed. */
  recipients: string[];
  /** The runtime keeps this channel's routes for every member on its own. */
  managed: boolean;
}

export interface TopoEdge {
  /** `<relation>:<from>:<to>`, stable across refreshes. */
  id: string;
  from: string;
  to: string;
  relation: TopoRelation;
  /** A chevron at `to`. */
  directed: boolean;
  state: TopoEdgeState;
  /** Traffic: messages in the window. */
  weight?: number;
  /** The management pair (endpoint ids) for connect and disconnect. */
  endpoints?: TopologyEdgeRef;
  /** Declared: the runtime wants the pair and it is not wired (a problem). */
  degraded?: boolean;
  /** Declared: an operator disconnected the pair on purpose. */
  suppressed?: boolean;
  operatorAdded?: boolean;
  /** Wired while an operator asked to disconnect it. */
  conflict?: boolean;
  /** The action in flight on a pending edge. */
  pendingAction?: TopologyMutationKind;
  /** Lineage: how the helper came from its parent. */
  lineage?: ConsoleTopologyLineageKind;
  /** Route: the routes behind the edge, and its channel. */
  routes?: ConsoleTopologyRoute[];
  channel?: string;
  managed?: boolean;
  failedDeliveries?: number;
  sentDeliveries?: number;
  /** Traffic: messages `from` to `to`, and `to` to `from`, and when the latest was sent. */
  counts?: { forward: number; backward: number };
  lastAtMs?: number;
  /** Traffic: the pair is also wired. */
  wired?: boolean;
}

export interface TopoGraph {
  /** Agent-like nodes: members, helpers, other teams' agents. */
  agents: TopoAgent[];
  byId: Map<string, TopoAgent>;
  sources: TopoSource[];
  stores: ConsoleTopologyStore[];
  edges: TopoEdge[];
  /** Distinct comms peers per node (wired, declared, cross and traffic). */
  degree: Record<string, number>;
  roles: string[];
  groups: string[];
  /** Maps any reference to a node (identity, member id, peer name, endpoint key) to its id. */
  resolve: (reference: string | null | undefined) => string | null;
}

export interface TopologyGraphInputs {
  /**
   * Every pair the topology query reports. When given (even empty), the
   * query decides what is wired; the nodes' own `wired_to` lists add no pair.
   */
  links?: readonly ConsoleTopologyLink[] | null;
  routes?: readonly ConsoleTopologyRoute[] | null;
  lineage?: readonly ConsoleTopologyLineage[] | null;
  /** Messages per ordered pair in the window. Omit to derive them from `frames`. */
  traffic?: readonly ConsoleTopologyTraffic[] | null;
  /** Frames to derive traffic from when `traffic` is omitted. */
  frames?: readonly ConsoleFrame[] | null;
  /** The traffic window: messages sent at or after this time count. */
  trafficSinceMs?: number;
  operations?: readonly TopologyOperationReceipt[] | null;
  stores?: readonly ConsoleTopologyStore[] | null;
}

const LINEAGE_LABELS: Array<{ key: string; kind: ConsoleTopologyLineageKind }> = [
  { key: "side_chat_of", kind: "side_chat" },
  { key: "spawned_by", kind: "spawn" },
  { key: "delegate_host_identity", kind: "delegate" },
  { key: "host_identity", kind: "spawn" },
  { key: "parent_identity", kind: "spawn" },
];

const PEER_TOOL_NAMES = new Set(["send_request", "send_message", "send_response"]);
/** Comms notice kinds that carry a message (a reply's notice reads response_terminal). */
const MESSAGE_COMMS_KINDS = new Set(["message", "request", "response", "response_terminal"]);
/** The frames that carry a tool call's result (live, and the session's saved copy). */
const TOOL_RESULT_EVENTS = new Set(["tool_result_received", "tool_execution_completed", "tool_result"]);
/**
 * A receiver-side delivery and a sender's call that no id ties to anything
 * (no receipt) within this span are one message.
 */
const MESSAGE_MATCH_MS = 15_000;

function textOf(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

/**
 * Whether a reference names an agent key, the sidebar's fuzzy match: equal,
 * equal without separators, a token of the reference ("team/role/key"), or
 * a run of tokens that spells the key.
 */
export function topologyReferenceMatchesKey(reference: string, key: string): boolean {
  const normalizedReference = reference.toLowerCase();
  const normalizedKey = key.toLowerCase();
  if (!normalizedKey) return false;
  if (normalizedReference === normalizedKey) return true;
  const compactReference = normalizedReference.replace(/[^a-z0-9]+/g, "");
  const compactKey = normalizedKey.replace(/[^a-z0-9]+/g, "");
  if (compactKey && compactReference === compactKey) return true;
  const tokens = normalizedReference.split(/[/:#\s]+/).filter(Boolean);
  if (tokens.includes(normalizedKey)) return true;
  if (!compactKey) return false;
  for (let start = 0; start < tokens.length; start++) {
    let compactSlice = "";
    for (let end = start; end < tokens.length; end++) {
      compactSlice += tokens[end]!.replace(/[^a-z0-9]+/g, "");
      if (compactSlice === compactKey) return true;
      if (compactSlice.length > compactKey.length) break;
    }
  }
  return false;
}

/** The canonical id of another team's agent. */
export function externalTopologyId(identity: string, authority: string): string {
  return `${identity}@${authority}`;
}

function parseEndpointKey(id: string): { authority: string; identity: string } | null {
  const parts = id.split("|");
  if (parts.length !== 3 || parts[0] !== "mk1") return null;
  try {
    const identity = decodeURIComponent(parts[2]!);
    return identity ? { authority: decodeURIComponent(parts[1]!), identity } : null;
  } catch {
    return null;
  }
}

interface NodeDraft {
  id: string;
  ref: TopologyEndpointRef;
  node?: ConsoleTopologyNode;
  agent?: ConsoleAgent;
  wiredRefs: Set<string>;
  external: boolean;
  authority?: string | null;
  /** Keys this node answers to (exact). */
  keys: Set<string>;
}

function isExternalNode(node: ConsoleTopologyNode): boolean {
  return node.presentation?.crossScope === true || node.labels?.topology_stub === "true";
}

function nodeIdentity(node: ConsoleTopologyNode): string {
  const parsed = node.identity ? parseEndpointKey(node.identity) : null;
  return (node.ref?.identity || parsed?.identity || node.identity || node.label || "").trim();
}

function nodeAuthority(node: ConsoleTopologyNode): string | null {
  const parsed = node.identity ? parseEndpointKey(node.identity) : null;
  return node.ref?.authority || parsed?.authority || null;
}

/**
 * Build the topology graph from the host's node sources and relation inputs.
 * Pure: the same inputs give the same graph (ids, order, edges).
 */
export function buildGraph(
  nodes: readonly ConsoleTopologyNode[],
  agents: readonly ConsoleAgent[],
  inputs: TopologyGraphInputs = {},
): TopoGraph {
  const agentByKey = new Map<string, ConsoleAgent>();
  for (const agent of agents) {
    for (const key of [agent.identity, agent.member_id, agent.agent_id]) {
      const trimmed = textOf(key);
      if (trimmed && !agentByKey.has(trimmed)) agentByKey.set(trimmed, agent);
    }
  }

  // 1. Node drafts, merged on the canonical id.
  const drafts = new Map<string, NodeDraft>();
  const order: string[] = [];
  const draftFor = (id: string, ref: TopologyEndpointRef, external: boolean, authority: string | null | undefined): NodeDraft => {
    let draft = drafts.get(id);
    if (!draft) {
      draft = { id, ref, wiredRefs: new Set(), external, authority, keys: new Set([id]) };
      drafts.set(id, draft);
      order.push(id);
    }
    return draft;
  };
  for (const node of nodes) {
    const identity = nodeIdentity(node);
    if (!identity) continue;
    const authority = nodeAuthority(node);
    const external = isExternalNode(node) && Boolean(authority);
    const id = external ? externalTopologyId(identity, authority!) : identity;
    const refId = (node.ref?.id || node.identity || id).trim();
    const draft = draftFor(id, { id: refId, authority, identity }, external, authority);
    if (!draft.node) draft.node = node;
    draft.keys.add(refId);
    if (node.identity) draft.keys.add(node.identity.trim());
    if (external) {
      draft.keys.add(`${authority}/${identity}`);
    } else {
      draft.keys.add(identity);
      const registry = agentByKey.get(identity) || agentByKey.get(refId);
      if (registry && !draft.agent) draft.agent = registry;
    }
    for (const peer of node.wired_to || []) {
      const trimmed = textOf(peer);
      if (trimmed) draft.wiredRefs.add(trimmed);
    }
  }
  for (const agent of agents) {
    const identity = textOf(agent.identity) || textOf(agent.member_id) || textOf(agent.agent_id);
    if (!identity) continue;
    // An agent the nodes already name merges into that node.
    const existing = drafts.get(identity)
      || [agent.member_id, agent.agent_id].map(textOf).filter(Boolean).map((key) => drafts.get(key)).find(Boolean);
    const draft = existing || draftFor(identity, { id: identity, identity }, false, null);
    if (!draft.agent) draft.agent = agent;
    for (const key of [agent.identity, agent.member_id, agent.agent_id]) {
      const trimmed = textOf(key);
      if (trimmed) draft.keys.add(trimmed);
    }
    for (const peer of agent.wired_to || []) {
      const trimmed = textOf(peer);
      if (trimmed) draft.wiredRefs.add(trimmed);
    }
  }

  // 2. The resolver: exact keys, then the last path segment, then the
  // sidebar's fuzzy match over identities. Never maps a bare identity to
  // another team's agent.
  const exact = new Map<string, string>();
  const folded = new Map<string, string>();
  const fuzzyKeys: Array<{ key: string; id: string }> = [];
  for (const draft of drafts.values()) {
    for (const key of draft.keys) {
      if (!exact.has(key)) exact.set(key, draft.id);
      const lower = key.toLowerCase();
      if (!folded.has(lower)) folded.set(lower, draft.id);
    }
    if (!draft.external) {
      for (const key of [draft.id, draft.agent?.member_id, draft.agent?.agent_id]) {
        const trimmed = textOf(key);
        if (trimmed && !trimmed.startsWith("mk1|")) fuzzyKeys.push({ key: trimmed, id: draft.id });
      }
    }
  }
  // Longer keys first, so "billing-refund-review" wins over "billing".
  fuzzyKeys.sort((left, right) => right.key.length - left.key.length || left.key.localeCompare(right.key));
  // A peer name starts with its team ("team/role/identity"): another team's
  // name must never resolve to a local agent of the same identity.
  const homeAuthorities = new Set<string>();
  const externalAuthorities = new Set<string>();
  for (const draft of drafts.values()) {
    if (!draft.authority) continue;
    (draft.external ? externalAuthorities : homeAuthorities).add(draft.authority);
  }
  const labelKeys = new Map<string, string>();
  const resolved = new Map<string, string | null>();
  const resolveUncached = (raw: string): string | null => {
    const direct = exact.get(raw) || folded.get(raw.toLowerCase());
    if (direct) return direct;
    const endpoint = parseEndpointKey(raw);
    if (endpoint) {
      if (externalAuthorities.has(endpoint.authority)) {
        return exact.get(externalTopologyId(endpoint.identity, endpoint.authority)) || null;
      }
      return exact.get(endpoint.identity) || null;
    }
    const segments = raw.split("/").filter(Boolean);
    if (segments.length > 1) {
      const team = segments[0]!;
      const tail = segments[segments.length - 1]!;
      if (externalAuthorities.has(team)) return exact.get(externalTopologyId(tail, team)) || null;
      // A team the map does not show is never a local agent of the same name.
      if (homeAuthorities.size > 0 && !homeAuthorities.has(team)) return null;
      const local = exact.get(tail) || folded.get(tail.toLowerCase());
      if (local) return local;
    }
    return fuzzyKeys.find((candidate) => topologyReferenceMatchesKey(raw, candidate.key))?.id
      || labelKeys.get(raw.toLowerCase())
      || null;
  };
  const resolve = (reference: string | null | undefined): string | null => {
    const raw = textOf(reference);
    if (!raw) return null;
    if (resolved.has(raw)) return resolved.get(raw)!;
    const match = resolveUncached(raw);
    resolved.set(raw, match);
    return match;
  };

  // 3. Agents.
  const byId = new Map<string, TopoAgent>();
  const list: TopoAgent[] = [];
  for (const id of order) {
    const draft = drafts.get(id)!;
    const node = draft.node;
    const registry = draft.agent;
    const labels: Record<string, string> = { ...(registry?.labels || {}), ...(node?.labels || {}) };
    const role = (node?.role || registry?.role || labels.role || "agent").trim();
    const group = (
      node?.group
      || registry?.group
      || labels.console_group
      || labels.group
      || labels.swarm_mob
      || node?.role
      || registry?.role
      || "Agents"
    ).trim();
    const identity = draft.ref.identity || id;
    const agent: TopoAgent = {
      id,
      ref: draft.ref,
      // One name for the agent on every surface; the raw key stays in tooltips.
      label: agentDisplayTitle({
        label: (node?.label || registry?.label || labels.display_name || identity).trim(),
        identity,
        member_id: registry?.member_id,
        agent_id: registry?.agent_id,
        labels,
        role,
        group,
      }),
      role,
      state: (registry?.state || node?.state || "").toLowerCase(),
      wiredTo: [],
      group,
      subgroup: node?.subgroup || registry?.subgroup || labels.shard || undefined,
      labels,
      presentation: node?.presentation,
      memberId: registry?.member_id,
      responsePhase: registry?.response_phase,
      agent: registry,
      kind: draft.external ? "external" : registry?.unobserved ? "unknown" : "agent",
      external: draft.external,
      authority: draft.authority,
    };
    byId.set(id, agent);
    list.push(agent);
    // A display name resolves only to a local agent; another team's names may repeat ours.
    if (!agent.external && !labelKeys.has(agent.label.toLowerCase())) labelKeys.set(agent.label.toLowerCase(), id);
  }

  // 4. Relations.
  const edges = new Map<string, TopoEdge>();
  const pairKey = (a: string, b: string) => topologyEdgeKey(a, b);
  const commsByPair = new Map<string, TopoEdge>();
  const putComms = (edge: TopoEdge) => {
    edges.set(edge.id, edge);
    commsByPair.set(pairKey(edge.from, edge.to), edge);
  };
  const commsEdge = (a: string, b: string, relation: "wired" | "cross", state: TopoEdgeState): TopoEdge => {
    const pair = canonicalTopologyEdge({ from: a, to: b });
    const fromAgent = byId.get(pair.from);
    const toAgent = byId.get(pair.to);
    return {
      id: `${relation}:${pair.from}:${pair.to}`,
      from: pair.from,
      to: pair.to,
      relation,
      directed: false,
      state,
      endpoints: fromAgent && toAgent ? canonicalTopologyEdge({ from: fromAgent.ref.id, to: toAgent.ref.id }) : undefined,
    };
  };

  const authoritative = Array.isArray(inputs.links);
  if (authoritative) {
    for (const link of inputs.links || []) {
      const a = resolve(link.from);
      const b = resolve(link.to);
      if (!a || !b || a === b) continue;
      const cross = link.cross === true || Boolean(byId.get(a)?.external || byId.get(b)?.external);
      const relation = cross ? "cross" : "wired";
      if (link.actual) {
        putComms({
          ...commsEdge(a, b, relation, "normal"),
          operatorAdded: link.operatorAdded || undefined,
          conflict: link.suppressed || undefined,
        });
      } else if (cross && link.desired) {
        // One runtime never attests the other team's half: an operator link
        // it wants is drawn as the link it is.
        putComms({ ...commsEdge(a, b, relation, "normal"), operatorAdded: link.operatorAdded || undefined });
      } else if (link.desired) {
        putComms({ ...commsEdge(a, b, relation, "declared"), degraded: true, operatorAdded: link.operatorAdded || undefined });
      } else if (link.suppressed && link.declared && !cross) {
        putComms({ ...commsEdge(a, b, relation, "declared"), suppressed: true });
      }
    }
  } else {
    for (const draft of drafts.values()) {
      for (const peer of draft.wiredRefs) {
        const other = resolve(peer);
        if (!other || other === draft.id) continue;
        if (commsByPair.has(pairKey(draft.id, other))) continue;
        const cross = Boolean(byId.get(draft.id)?.external || byId.get(other)?.external);
        putComms(commsEdge(draft.id, other, cross ? "cross" : "wired", "normal"));
      }
    }
  }
  for (const edge of commsByPair.values()) {
    if (edge.state === "declared") continue;
    byId.get(edge.from)?.wiredTo.push(edge.to);
    byId.get(edge.to)?.wiredTo.push(edge.from);
  }

  // Operations in flight: the pair's edge turns pending; a connect between a
  // pair with no edge yet draws one.
  for (const operation of inputs.operations || []) {
    if (!operation.edge || !topologyOperationIsPending(operation)) continue;
    const a = resolve(operation.edge.from);
    const b = resolve(operation.edge.to);
    if (!a || !b || a === b) continue;
    const existing = commsByPair.get(pairKey(a, b));
    if (existing) {
      existing.state = "pending";
      existing.pendingAction = operation.action;
    } else if (operation.action !== "disconnect") {
      const cross = Boolean(byId.get(a)?.external || byId.get(b)?.external);
      putComms({ ...commsEdge(a, b, cross ? "cross" : "wired", "pending"), pendingAction: operation.action });
    }
  }

  // Lineage: typed (mob events) first, then the labels, one parent per child.
  const lineageFor = new Map<string, { parent: string; kind: ConsoleTopologyLineageKind; atMs?: number | null }>();
  for (const entry of inputs.lineage || []) {
    const child = resolve(entry.child);
    const parent = resolve(entry.parent);
    if (!child || !parent || child === parent || lineageFor.has(child)) continue;
    lineageFor.set(child, { parent, kind: entry.kind, atMs: entry.atMs });
  }
  for (const agent of list) {
    if (lineageFor.has(agent.id)) continue;
    const sideChatOf = textOf(agent.agent?.side_chat_of);
    const candidates: Array<{ value: string; kind: ConsoleTopologyLineageKind }> = sideChatOf
      ? [{ value: sideChatOf, kind: "side_chat" }]
      : [];
    for (const { key, kind } of LINEAGE_LABELS) {
      const value = textOf(agent.labels[key]);
      if (value) candidates.push({ value, kind });
    }
    for (const candidate of candidates) {
      const parent = resolve(candidate.value);
      if (parent && parent !== agent.id) {
        lineageFor.set(agent.id, { parent, kind: candidate.kind });
        break;
      }
    }
  }
  for (const [child, { parent, kind }] of lineageFor) {
    const agent = byId.get(child);
    if (!agent) continue;
    agent.parent = parent;
    agent.lineage = kind;
    if (agent.kind === "agent" || agent.kind === "unknown") agent.kind = "helper";
    edges.set(`lineage:${parent}:${child}`, {
      id: `lineage:${parent}:${child}`,
      from: parent,
      to: child,
      relation: "lineage",
      directed: true,
      state: "normal",
      lineage: kind,
    });
  }

  // Routes: a channel feeds each recipient on the map.
  const sourceById = new Map<string, TopoSource>();
  const routeEdges = new Map<string, TopoEdge>();
  for (const route of inputs.routes || []) {
    const channel = textOf(route.channel) || "default";
    const recipient = resolve(route.recipient);
    if (!recipient) continue;
    const sourceId = `channel:${channel}`;
    let source = sourceById.get(sourceId);
    if (!source) {
      source = { id: sourceId, channel, label: channel, routes: [], recipients: [], managed: true };
      sourceById.set(sourceId, source);
    }
    source.routes.push(route);
    if (!source.recipients.includes(recipient)) source.recipients.push(recipient);
    if (!route.managed) source.managed = false;
    const id = `route:${sourceId}:${recipient}`;
    let edge = routeEdges.get(id);
    if (!edge) {
      edge = {
        id,
        from: sourceId,
        to: recipient,
        relation: "route",
        directed: true,
        state: "normal",
        routes: [],
        channel,
        managed: true,
        failedDeliveries: 0,
        sentDeliveries: 0,
      };
      routeEdges.set(id, edge);
    }
    edge.routes!.push(route);
    if (!route.managed) edge.managed = false;
    edge.failedDeliveries! += route.deliveries?.failed || 0;
    edge.sentDeliveries! += route.deliveries?.sent || 0;
    if ((route.deliveries?.lastAtMs || 0) > (edge.lastAtMs || 0)) edge.lastAtMs = route.deliveries!.lastAtMs!;
  }
  for (const edge of routeEdges.values()) {
    if (edge.failedDeliveries) edge.state = "failed";
    edges.set(edge.id, edge);
  }

  const roles = Array.from(new Set(list.map((agent) => agent.role))).sort((a, b) => a.localeCompare(b));
  const groups = Array.from(new Set(list.map((agent) => agent.group))).sort((a, b) => a.localeCompare(b));
  for (const agent of list) agent.wiredTo.sort();
  const base: TopoGraph = {
    agents: list,
    byId,
    sources: Array.from(sourceById.values()).sort((a, b) => a.label.localeCompare(b.label)),
    stores: [...(inputs.stores || [])],
    edges: sortEdges(Array.from(edges.values())),
    degree: degreeOf(list, edges.values()),
    roles,
    groups,
    resolve,
  };
  if (!inputs.traffic && !inputs.frames) return base;
  const traffic = inputs.traffic
    ? inputs.traffic
    : deriveTopologyTraffic(inputs.frames || [], resolve, inputs.trafficSinceMs ?? 0).traffic;
  return withTopologyTraffic(base, traffic);
}

const RELATION_ORDER: Record<TopoRelation, number> = { route: 0, lineage: 1, wired: 2, cross: 3, traffic: 4 };

function sortEdges(edges: TopoEdge[]): TopoEdge[] {
  return edges.sort((left, right) =>
    RELATION_ORDER[left.relation] - RELATION_ORDER[right.relation] || left.id.localeCompare(right.id)
  );
}

/** Distinct comms peers per node: wired, declared, cross and traffic (not routes or lineage). */
function degreeOf(agents: readonly TopoAgent[], edges: Iterable<TopoEdge>): Record<string, number> {
  const peers = new Map<string, Set<string>>();
  for (const edge of edges) {
    if (edge.relation === "route" || edge.relation === "lineage") continue;
    if (!peers.has(edge.from)) peers.set(edge.from, new Set());
    if (!peers.has(edge.to)) peers.set(edge.to, new Set());
    peers.get(edge.from)!.add(edge.to);
    peers.get(edge.to)!.add(edge.from);
  }
  const degree: Record<string, number> = {};
  for (const agent of agents) degree[agent.id] = peers.get(agent.id)?.size || 0;
  return degree;
}

/**
 * The graph with these messages as its traffic edges (replacing any it had):
 * one edge per pair, directed when the messages went one way, weighted by
 * their count, marked when the pair is also wired.
 */
export function withTopologyTraffic(graph: TopoGraph, traffic: readonly ConsoleTopologyTraffic[]): TopoGraph {
  const pairKey = (a: string, b: string) => topologyEdgeKey(a, b);
  const edges = graph.edges.filter((edge) => edge.relation !== "traffic");
  const commsByPair = new Map<string, TopoEdge>();
  for (const edge of edges) {
    if (edge.relation === "wired" || edge.relation === "cross") commsByPair.set(pairKey(edge.from, edge.to), edge);
  }
  const trafficByPair = new Map<string, { a: string; b: string; forward: number; backward: number; lastAtMs: number }>();
  for (const entry of traffic) {
    const from = graph.resolve(entry.from);
    const to = graph.resolve(entry.to);
    if (!from || !to || from === to || entry.count <= 0) continue;
    const pair = canonicalTopologyEdge({ from, to });
    const key = pairKey(from, to);
    const bucket = trafficByPair.get(key) || { a: pair.from, b: pair.to, forward: 0, backward: 0, lastAtMs: 0 };
    if (from === bucket.a) bucket.forward += entry.count;
    else bucket.backward += entry.count;
    bucket.lastAtMs = Math.max(bucket.lastAtMs, entry.lastAtMs || 0);
    trafficByPair.set(key, bucket);
  }
  for (const [key, bucket] of trafficByPair) {
    const oneWay = bucket.forward === 0 || bucket.backward === 0;
    const from = oneWay && bucket.forward === 0 ? bucket.b : bucket.a;
    const to = from === bucket.a ? bucket.b : bucket.a;
    const forward = from === bucket.a ? bucket.forward : bucket.backward;
    const backward = from === bucket.a ? bucket.backward : bucket.forward;
    const comms = commsByPair.get(key);
    edges.push({
      id: `traffic:${from}:${to}`,
      from,
      to,
      relation: "traffic",
      directed: oneWay,
      state: "normal",
      weight: forward + backward,
      counts: { forward, backward },
      lastAtMs: bucket.lastAtMs || undefined,
      wired: Boolean(comms && comms.state !== "declared"),
      endpoints: comms?.endpoints,
    });
  }
  return { ...graph, edges: sortEdges(edges), degree: degreeOf(graph.agents, edges) };
}


// ---------------------------------------------------------------------------
// Traffic from frames
// ---------------------------------------------------------------------------

export interface TopologyTrafficResult {
  /** Messages per ordered pair, sent at or after the window start. */
  traffic: ConsoleTopologyTraffic[];
  /** Each message, newest last (particles play the last few seconds of them). */
  messages: ConsoleTopologyMessage[];
}

function frameData(frame: ConsoleFrame): Record<string, unknown> | null {
  return recordOf(frame.data);
}

function toolName(data: Record<string, unknown> | null): string {
  return textOf(data?.name) || textOf(data?.tool_name);
}

interface PeerToolCall {
  id: string;
  name: string;
  args: Record<string, unknown> | null;
}

function peerToolCalls(data: Record<string, unknown> | null): PeerToolCall[] {
  if (!data) return [];
  const calls: PeerToolCall[] = [];
  const direct = toolName(data);
  if (direct) {
    calls.push({ id: textOf(data.tool_call_id) || textOf(data.id), name: direct, args: recordOf(data.args) });
  }
  const message = recordOf(data.message);
  const blocks = Array.isArray(message?.blocks) ? message!.blocks as unknown[] : [];
  for (const block of blocks) {
    const record = recordOf(block);
    if (!record || textOf(record.block_type) !== "tool_use") continue;
    const tool = recordOf(record.data);
    const name = toolName(tool);
    if (name) calls.push({ id: textOf(tool?.id), name, args: recordOf(tool?.args) });
  }
  return calls.filter((call) => PEER_TOOL_NAMES.has(call.name));
}

interface PeerCallOutcome {
  /** The call refused or failed: nothing was sent. */
  refused: boolean;
  /** The envelope id the call's receipt names: the recipient's delivery id. */
  envelope: string;
}

/** A tool result frame's call id and outcome (refusal, or the receipt's envelope id). */
function peerCallOutcome(data: Record<string, unknown> | null): { id: string; outcome: PeerCallOutcome } | null {
  if (!data) return null;
  const id = textOf(data.tool_call_id) || textOf(data.id);
  if (!id) return null;
  const texts = [
    typeof data.result === "string" ? data.result : "",
    ...(Array.isArray(data.content) ? data.content.map((block) => textOf(recordOf(block)?.text)) : []),
  ];
  let refused = data.is_error === true;
  let envelope = "";
  for (const text of texts) {
    if (!text.startsWith("{")) continue;
    let parsed: Record<string, unknown> | null = null;
    try {
      parsed = recordOf(JSON.parse(text));
    } catch {
      parsed = null;
    }
    if (!parsed) continue;
    if (parsed.error) refused = true;
    envelope = envelope || textOf(recordOf(parsed.receipt)?.envelope_id);
  }
  return { id, outcome: { refused, envelope } };
}

function commsBlocks(data: Record<string, unknown> | null): Array<Record<string, unknown>> {
  const blocks: Array<Record<string, unknown>> = [];
  if (!data) return blocks;
  const seen = new Set<string>();
  const visit = (candidate: unknown) => {
    const record = recordOf(candidate);
    if (!record) return;
    if (textOf(record.type) === "comms") {
      const key = textOf(record.delivery_id) || textOf(record.request_id) || JSON.stringify(record.peer || null);
      if (!seen.has(key)) {
        seen.add(key);
        blocks.push(record);
      }
    }
    if (Array.isArray(record.blocks)) for (const block of record.blocks) visit(block);
  };
  visit(data);
  visit(data.message);
  return blocks;
}

function registerPeer(registry: Map<string, string>, id: unknown, name: unknown): void {
  const peerId = textOf(id);
  const peerName = textOf(name);
  if (peerId && peerName && peerId !== peerName) registry.set(peerId, peerName);
}

function capturePeers(registry: Map<string, string>, frame: ConsoleFrame, data: Record<string, unknown> | null): void {
  if (!data) return;
  const peer = recordOf(data.peer);
  if (peer) registerPeer(registry, peer.id, peer.display_name);
  for (const block of commsBlocks(data)) {
    const blockPeer = recordOf(block.peer);
    if (blockPeer) registerPeer(registry, blockPeer.id, blockPeer.display_name);
    const spec = recordOf(recordOf(block.payload)?.peer_spec);
    if (spec) registerPeer(registry, spec.peer_id, spec.name);
  }
  if (toolName(data) === "peers" && (frame.event === "tool_execution_completed" || frame.event === "tool_result_received")) {
    const raw = typeof data.result === "string" ? data.result : (() => {
      try {
        return JSON.stringify(data.result ?? "");
      } catch {
        return "";
      }
    })();
    try {
      const parsed = JSON.parse(raw) as { peers?: Array<{ peer_id?: unknown; name?: unknown }> };
      for (const entry of Array.isArray(parsed.peers) ? parsed.peers : []) registerPeer(registry, entry.peer_id, entry.name);
    } catch {
      // Not a JSON peers listing.
    }
  }
}

/**
 * Peer messages between agents, from real frames: a delivered message on the
 * receiving side (a comms notice or an ingested peer message, one per
 * delivery) and a send tool call on the sending side (counted only when no
 * delivery of it was seen). Lifecycle notices are not messages. A message
 * whose ends do not both resolve is left out.
 */
export function deriveTopologyTraffic(
  frames: readonly ConsoleFrame[],
  resolve: (reference: string | null | undefined) => string | null,
  sinceMs = 0,
): TopologyTrafficResult {
  const registry = new Map<string, string>();
  // Each send call's outcome, by call id: a refused call sent nothing, and a
  // receipt's envelope id is the delivery id of the recipient's notice.
  const outcomes = new Map<string, PeerCallOutcome>();
  for (const frame of frames) {
    const data = frameData(frame);
    capturePeers(registry, frame, data);
    if (!TOOL_RESULT_EVENTS.has(frame.event)) continue;
    const result = peerCallOutcome(data);
    if (!result) continue;
    const known = outcomes.get(result.id);
    outcomes.set(result.id, {
      refused: Boolean(known?.refused) || result.outcome.refused,
      envelope: known?.envelope || result.outcome.envelope,
    });
  }
  const peerName = (value: unknown): string => {
    const raw = textOf(value);
    return registry.get(raw) || raw;
  };

  interface Message { id: string; from: string; to: string; atMs: number; side: "delivery" | "call"; envelope?: string }
  const deliveries = new Map<string, Message>();
  const calls = new Map<string, Message>();
  for (const frame of frames) {
    const atMs = frame.timestampMs || 0;
    if (!atMs || atMs < sinceMs) continue;
    const data = frameData(frame);
    const self = frame.identity ? resolve(frame.identity) : null;
    if (self && (frame.event === "system_notice" || frame.event === "peer_content_ingested")) {
      const candidates = frame.event === "peer_content_ingested" && data && MESSAGE_COMMS_KINDS.has(textOf(data.kind))
        ? [{ ...data, direction: data.direction ?? "incoming" }]
        : commsBlocks(data);
      for (const block of candidates) {
        if (!MESSAGE_COMMS_KINDS.has(textOf(block.kind))) continue;
        const peer = recordOf(block.peer) || {};
        const other = resolve(peerName(peer.display_name) || peerName(peer.id)) || resolve(peerName(peer.id));
        if (!other || other === self) continue;
        const outgoing = textOf(block.direction) === "outgoing";
        const from = outgoing ? self : other;
        const to = outgoing ? other : self;
        const key = textOf(block.delivery_id) || textOf(block.request_id) || `${frame.id}:${from}>${to}`;
        if (!deliveries.has(key)) deliveries.set(key, { id: key, from, to, atMs, side: "delivery" });
      }
    }
    const readsCalls = frame.event === "tool_call_requested"
      || frame.event === "tool_call"
      || frame.event === "tool_execution_started"
      || frame.event === "interaction_complete";
    if (self && readsCalls) {
      for (const call of peerToolCalls(data)) {
        const args = call.args;
        const references = [
          peerName(args?.peer_id),
          ...["to", "identity", "target_identity", "recipient", "display_name"].map((key) => textOf(args?.[key])),
        ].filter(Boolean);
        let recipient: string | null = null;
        for (const reference of references) {
          recipient = resolve(reference);
          if (recipient) break;
        }
        if (!recipient || recipient === self) continue;
        const outcome = call.id ? outcomes.get(call.id) : undefined;
        if (outcome?.refused) continue;
        const key = call.id ? `call:${call.id}` : `call:${frame.id}:${recipient}`;
        const existing = calls.get(key);
        if (!existing || existing.atMs > atMs) {
          calls.set(key, { id: key, from: self, to: recipient, atMs, side: "call", envelope: outcome?.envelope || undefined });
        }
      }
    }
  }

  const messages: Message[] = [...deliveries.values()];
  // Ends tie by ids first: a call's receipt names the delivery id of the
  // recipient's notice. A receipted call whose notice is not in view still
  // counts once; it never takes another delivery.
  const claimed = new Set<string>();
  const unreceipted: Message[] = [];
  for (const call of calls.values()) {
    if (!call.envelope) {
      unreceipted.push(call);
      continue;
    }
    if (claimed.has(call.envelope)) continue;
    claimed.add(call.envelope);
    if (!deliveries.has(call.envelope)) messages.push({ ...call, id: call.envelope });
  }
  // Only the ends no id ties pair by time: a call without a receipt and a
  // delivery no call claimed, the same pair, within the span.
  const deliveredByPair = new Map<string, number[]>();
  for (const message of deliveries.values()) {
    if (claimed.has(message.id)) continue;
    const key = `${message.from}>${message.to}`;
    if (!deliveredByPair.has(key)) deliveredByPair.set(key, []);
    deliveredByPair.get(key)!.push(message.atMs);
  }
  for (const call of unreceipted) {
    const times = deliveredByPair.get(`${call.from}>${call.to}`) || [];
    const matched = times.findIndex((at) => Math.abs(at - call.atMs) <= MESSAGE_MATCH_MS);
    if (matched >= 0) {
      // The receiving side already counted this message.
      times.splice(matched, 1);
      continue;
    }
    messages.push(call);
  }
  messages.sort((left, right) => left.atMs - right.atMs || left.id.localeCompare(right.id));

  const byPair = new Map<string, ConsoleTopologyTraffic>();
  for (const message of messages) {
    const key = `${message.from}>${message.to}`;
    const entry = byPair.get(key) || { from: message.from, to: message.to, count: 0, lastAtMs: 0 };
    entry.count += 1;
    entry.lastAtMs = Math.max(entry.lastAtMs || 0, message.atMs);
    byPair.set(key, entry);
  }
  return {
    traffic: Array.from(byPair.values()),
    messages: messages.map(({ id, from, to, atMs }) => ({ id, from, to, atMs })),
  };
}

// ---------------------------------------------------------------------------
// Live activity from frames
// ---------------------------------------------------------------------------

export interface TopoActivity {
  /** Identity: the newest frame's time (any frame). */
  active: Record<string, number>;
  /**
   * Identity: a run is open (between `run_started`/`interaction_started` and
   * the run's end). Sticky until the end arrives.
   */
  busy: Record<string, boolean>;
}

const RUN_START_EVENTS = new Set(["interaction_started", "run_started"]);
const RUN_END_EVENTS = new Set(["interaction_complete", "interaction_failed", "run_completed", "run_failed", "run_canceled"]);

/** Who has a run open and who was active recently, from the frames (newest first). */
export function deriveTopologyActivity(
  frames: readonly ConsoleFrame[],
  graph: Pick<TopoGraph, "resolve">,
): TopoActivity {
  const active: Record<string, number> = {};
  const busy: Record<string, boolean> = {};
  for (let index = frames.length - 1; index >= 0; index -= 1) {
    const frame = frames[index]!;
    const at = frame.timestampMs || 0;
    if (!at || !frame.identity) continue;
    const id = graph.resolve(frame.identity);
    if (!id) continue;
    if ((active[id] || 0) < at) active[id] = at;
    if (RUN_START_EVENTS.has(frame.event)) busy[id] = true;
    else if (RUN_END_EVENTS.has(frame.event)) busy[id] = false;
  }
  return { active, busy };
}

/** `deriveTopologyActivity`, memoized on the frames and the graph. */
export function useTopologyActivity(
  frames: readonly ConsoleFrame[],
  graph: Pick<TopoGraph, "resolve">,
): TopoActivity {
  return React.useMemo(() => deriveTopologyActivity(frames, graph), [frames, graph]);
}

/** The stable key of an undirected pair. */
export function edgeKey(a: string, b: string): string {
  return topologyEdgeKey(a, b);
}
