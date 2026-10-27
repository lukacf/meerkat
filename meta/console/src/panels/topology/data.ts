// Shared topology data shaping + real-activity wiring.
//
// The console only exposes a flat node list (`identity, label, role, state,
// wired_to`). All four reference layouts are happy with that — what they
// actually need is:
//   * agents[] keyed by identity
//   * edges[] derived from `wired_to` (mirrored to undirected pairs)
//   * degree[identity] (so radius/ring-bucket can scale by hub-ness)
//   * roles[] in display order
//
// The "real pulses" layer reads the live activity stream:
//   * any frame with a known `identity` marks that node hot for `life` ms
//   * a `send_*` tool call with a resolvable `peer_id` becomes a directed
//     pulse along the (sender → recipient) edge
//   * the peer-id-to-identity registry is built from `peers` tool results
//     observed in the same stream — same trick the chat adapter uses
//
// No synthetic event bus, no fabricated traffic.

import React from "react";
import type { ResponsePhase } from "@console-core";
import type { ConsoleAgent, ConsoleFrame, ConsoleTopologyNode } from "../../types";

export interface TopoAgent {
  id: string;
  label: string;
  role: string;
  state: string;
  wiredTo: string[];
  group: string;
  subgroup?: string;
  labels: Record<string, string>;
  memberId?: string;
  responsePhase?: ResponsePhase;
}

export interface TopoEdge {
  from: string;
  to: string;
}

export interface TopoGraph {
  agents: TopoAgent[];
  byId: Map<string, TopoAgent>;
  edges: TopoEdge[];
  degree: Record<string, number>;
  roles: string[];
  groups: string[];
}

export interface TopoGraphStats {
  nodeCount: number;
  edgeCount: number;
  possibleEdges: number;
  density: number;
  minDegree: number;
  maxDegree: number;
  avgDegree: number;
  isolatedCount: number;
}

export interface TopoGroupSummary {
  group: string;
  count: number;
  internalEdges: number;
  externalEdges: number;
}

export interface TopoGroupMatrixCell {
  from: string;
  to: string;
  edges: number;
}

export interface TopoPulse {
  id: string;
  from: string;
  to: string;
  ts: number;
}

export interface TopoActivity {
  /// identity → most-recent-frame ts (any frame). Decays over `life` ms.
  /// Used to draw the soft "recent activity" halo.
  active: Record<string, number>;
  pulses: TopoPulse[];
  /// identity → currently-running-a-turn boolean. Sticky between
  /// `interaction_started`/`run_started` and the matching
  /// `interaction_complete`/`interaction_failed`/`run_completed`/`run_failed`/
  /// `run_canceled`. Drives the persistent "working" spinner ring.
  busy: Record<string, boolean>;
  calls: Record<string, number>;
}

const PEER_TOOL_NAMES = new Set(["send_request", "send_message", "send_response"]);

function frameData(frame: ConsoleFrame): Record<string, unknown> | null {
  return frame.data && typeof frame.data === "object"
    ? frame.data as Record<string, unknown>
    : null;
}

function toolName(data: Record<string, unknown> | null): string {
  if (!data) return "";
  if (typeof data.name === "string") return data.name;
  if (typeof data.tool_name === "string") return data.tool_name;
  return "";
}

interface TopologyToolCall {
  id: string;
  name: string;
  args: Record<string, unknown> | null;
}

function toolCallsFromFrameData(data: Record<string, unknown> | null): TopologyToolCall[] {
  if (!data) return [];
  const calls: TopologyToolCall[] = [];
  const directName = toolName(data);
  if (directName) {
    calls.push({
      id: textFromUnknown(data.id),
      name: directName,
      args: data.args && typeof data.args === "object" ? data.args as Record<string, unknown> : null,
    });
  }

  const message = data.message && typeof data.message === "object"
    ? data.message as Record<string, unknown>
    : null;
  const blocks = Array.isArray(message?.blocks) ? message.blocks : [];
  for (const block of blocks) {
    if (!block || typeof block !== "object") continue;
    const blockRecord = block as Record<string, unknown>;
    if (textFromUnknown(blockRecord.block_type) !== "tool_use") continue;
    const toolData = blockRecord.data && typeof blockRecord.data === "object"
      ? blockRecord.data as Record<string, unknown>
      : null;
    const name = toolName(toolData);
    if (!name) continue;
    calls.push({
      id: textFromUnknown(toolData?.id),
      name,
      args: toolData?.args && typeof toolData.args === "object" ? toolData.args as Record<string, unknown> : null,
    });
  }
  return calls;
}

function resultText(value: unknown): string {
  if (typeof value === "string") return value;
  if (!value || typeof value !== "object") return "";
  try {
    return JSON.stringify(value);
  } catch {
    return "";
  }
}

function peerLastSegment(value: string): string {
  return value.split("/").filter(Boolean).pop() || value;
}

function textFromUnknown(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function capturePeerRegistry(peerRegistry: Map<string, string>, rawResult: unknown): void {
  const raw = resultText(rawResult);
  if (!raw) return;
  try {
    const parsed = JSON.parse(raw) as { peers?: Array<{ peer_id?: unknown; name?: unknown }> };
    if (!Array.isArray(parsed.peers)) return;
    for (const peer of parsed.peers) {
      if (typeof peer.peer_id === "string" && typeof peer.name === "string") {
        peerRegistry.set(peer.peer_id, peer.name);
      }
    }
  } catch {
    // Ignore non-JSON peers payloads.
  }
}

function resolvePeerTarget(
  args: Record<string, unknown> | null,
  peerRegistry: Map<string, string>,
  graph: TopoGraph,
): string | null {
  const candidates: string[] = [];
  const peerId = typeof args?.peer_id === "string" ? args.peer_id.trim() : "";
  const registryName = peerId ? peerRegistry.get(peerId) : "";
  if (registryName) candidates.push(registryName, peerLastSegment(registryName));
  for (const key of ["identity", "target_identity", "recipient", "to", "display_name"]) {
    const value = typeof args?.[key] === "string" ? args[key].trim() : "";
    if (value) candidates.push(value, peerLastSegment(value));
  }
  if (peerId) candidates.push(peerId);

  for (const candidate of candidates) {
    if (graph.byId.has(candidate)) return candidate;
    const match = graph.agents.find((agent) =>
      agent.id === candidate ||
      agent.label === candidate ||
      agent.memberId === candidate
    );
    if (match) return match.id;
  }
  return null;
}

function resolveGraphIdentity(value: string, graph: TopoGraph): string | null {
  const raw = value.trim();
  if (!raw) return null;
  const candidates = [raw, peerLastSegment(raw)];
  for (const candidate of candidates) {
    if (graph.byId.has(candidate)) return candidate;
    const match = graph.agents.find((agent) =>
      agent.id === candidate ||
      agent.label === candidate ||
      agent.memberId === candidate
    );
    if (match) return match.id;
  }
  return null;
}

function commsBlocksFromFrameData(data: Record<string, unknown> | null): Array<Record<string, unknown>> {
  const candidates: unknown[] = [];
  if (data) {
    candidates.push(data);
    if (data.message && typeof data.message === "object") candidates.push(data.message);
  }
  const blocks: Array<Record<string, unknown>> = [];
  for (const candidate of candidates) {
    if (!candidate || typeof candidate !== "object") continue;
    const record = candidate as Record<string, unknown>;
    const recordKind = textFromUnknown(record.kind);
    if (recordKind === "comms") blocks.push(record);
    if (!Array.isArray(record.blocks)) continue;
    for (const block of record.blocks) {
      if (!block || typeof block !== "object") continue;
      const blockRecord = block as Record<string, unknown>;
      if (textFromUnknown(blockRecord.type) === "comms") blocks.push(blockRecord);
    }
  }
  return blocks;
}

function typedCommsPulseFromFrame(
  frame: ConsoleFrame,
  data: Record<string, unknown> | null,
  graph: TopoGraph,
): TopoPulse | null {
  if (frame.event !== "system_notice") return null;
  const receiver = frame.identity ? resolveGraphIdentity(frame.identity, graph) : null;
  if (!receiver) return null;
  const blocks = commsBlocksFromFrameData(data);
  for (const block of blocks) {
    const peer = block.peer && typeof block.peer === "object"
      ? block.peer as Record<string, unknown>
      : {};
    const peerIdentity = resolveGraphIdentity(
      textFromUnknown(peer.display_name) || textFromUnknown(peer.id),
      graph,
    );
    if (!peerIdentity || peerIdentity === receiver) continue;
    const direction = textFromUnknown(block.direction) || "incoming";
    const requestId = textFromUnknown(block.request_id);
    if (direction === "outgoing") {
      return {
        id: requestId || `${frame.id || frame.timestampMs}-typed-comms`,
        from: receiver,
        to: peerIdentity,
        ts: frame.timestampMs || 0,
      };
    }
    return {
      id: requestId || `${frame.id || frame.timestampMs}-typed-comms`,
      from: peerIdentity,
      to: receiver,
      ts: frame.timestampMs || 0,
    };
  }
  return null;
}

const ROLE_ORDER_HINT = [
  "user",
  "personal",
  "coordinator",
  "scribe",
  "review",
  "summarizer",
  "initiative",
  "channel",
  "responder",
  "domain",
  "internal",
  "approval",
  "monitor",
];

/// Stable colour per role. Six-slot palette cycles when we run past the
/// well-known names — keeps the legend deterministic.
export const ROLE_PALETTE = [
  "var(--focus)",
  "var(--accent)",
  "var(--ok)",
  "var(--warn)",
  "var(--c-init)",
  "var(--ink-muted)",
];

export function colourForRole(role: string, roleIndex: Record<string, number>): string {
  const idx = roleIndex[role] ?? 0;
  return ROLE_PALETTE[idx % ROLE_PALETTE.length];
}

function roleSortKey(role: string): number {
  const idx = ROLE_ORDER_HINT.findIndex((hint) => role.toLowerCase().includes(hint));
  return idx === -1 ? ROLE_ORDER_HINT.length : idx;
}

/// Build the canonical graph from the console's flat node list, falling
/// back to the agent registry when the topology snapshot is empty (the
/// gateway sometimes ships only one or the other).
export function buildGraph(
  nodes: ConsoleTopologyNode[],
  agents: ConsoleAgent[],
): TopoGraph {
  const agentByIdentity = new Map<string, ConsoleAgent>();
  for (const a of agents) {
    const candidates = [a.identity, a.member_id, a.agent_id].filter(Boolean) as string[];
    for (const id of candidates) {
      if (!agentByIdentity.has(id)) agentByIdentity.set(id, a);
    }
  }

  const source: ConsoleTopologyNode[] = nodes.length > 0
    ? nodes
    : agents.map((a) => ({
        identity: a.identity || a.member_id,
        label: a.label,
        role: a.role,
        state: a.state,
        wired_to: a.wired_to,
        labels: a.labels,
        group: a.group,
        subgroup: a.subgroup,
      }));

  const byId = new Map<string, TopoAgent>();
  const list: TopoAgent[] = [];
  for (const n of source) {
    const id = (n.identity || n.label || "").trim();
    if (!id || byId.has(id)) continue;
    const registry = agentByIdentity.get(id);
    const labels = {
      ...(registry?.labels || {}),
      ...(n.labels || {}),
    };
    const group = (
      n.group
      || registry?.group
      || labels.console_group
      || labels.group
      || labels.swarm_mob
      || n.role
      || registry?.role
      || "Agents"
    ).trim();
    const agent: TopoAgent = {
      id,
      label: (n.label || registry?.label || labels.display_name || id).trim(),
      role: (n.role || registry?.role || labels.role || "agent").trim(),
      state: (n.state || registry?.state || "").toLowerCase(),
      wiredTo: (n.wired_to || []).map((s) => s.trim()).filter(Boolean),
      group,
      subgroup: n.subgroup || registry?.subgroup || labels.shard || undefined,
      labels,
      memberId: registry?.member_id,
      responsePhase: registry?.response_phase,
    };
    byId.set(id, agent);
    list.push(agent);
  }

  // Edges: mirror wired_to into a deduped pair set so each pair shows up
  // exactly once. The console's `wired_to` is asymmetric (each side lists
  // its peers) but visually the wire is undirected, so we collapse on the
  // sorted-tuple key.
  const seen = new Set<string>();
  const edges: TopoEdge[] = [];
  for (const a of list) {
    for (const t of a.wiredTo) {
      if (!byId.has(t) || t === a.id) continue;
      const key = a.id < t ? `${a.id}|${t}` : `${t}|${a.id}`;
      if (seen.has(key)) continue;
      seen.add(key);
      edges.push({ from: a.id, to: t });
    }
  }

  const degree: Record<string, number> = {};
  for (const e of edges) {
    degree[e.from] = (degree[e.from] || 0) + 1;
    degree[e.to] = (degree[e.to] || 0) + 1;
  }

  const roles = Array.from(new Set(list.map((a) => a.role))).sort((a, b) => {
    const ra = roleSortKey(a);
    const rb = roleSortKey(b);
    if (ra !== rb) return ra - rb;
    return a.localeCompare(b);
  });
  const groups = Array.from(new Set(list.map((a) => a.group))).sort((a, b) => {
    const ca = list.filter((agent) => agent.group === a).length;
    const cb = list.filter((agent) => agent.group === b).length;
    if (ca !== cb) return cb - ca;
    return a.localeCompare(b);
  });

  return { agents: list, byId, edges, degree, roles, groups };
}

export function roleIndexFor(roles: string[]): Record<string, number> {
  const idx: Record<string, number> = {};
  roles.forEach((r, i) => { idx[r] = i; });
  return idx;
}

/// Live activity hook — reads the global activity stream and produces:
///   * `active`: identity → most-recent-frame-ts (used for halos)
///   * `pulses`: list of in-flight directional pulses derived from
///     resolvable peer-comms tool calls
///
/// "Resolvable" means we've seen a `peers` tool result frame in the
/// stream that maps the recipient's `peer_id` (UUID) back to a known
/// identity. Until that mapping arrives, peer pulses for that pair are
/// silently dropped — we don't fabricate a direction we can't prove.
export function useTopologyActivity(
  frames: ConsoleFrame[],
  graph: TopoGraph,
  options: { life?: number } = {},
): TopoActivity {
  const life = options.life ?? 1500;
  const [now, setNow] = React.useState(() => Date.now());

  // Heartbeat that drives pulse-fade animation. Stops when no pulses are
  // in flight — checked via the ref to avoid pinning re-renders.
  const ticking = React.useRef(false);
  React.useEffect(() => {
    if (ticking.current) return;
    let raf = 0;
    let stopped = false;
    ticking.current = true;
    const step = () => {
      if (stopped) return;
      setNow(Date.now());
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => {
      stopped = true;
      ticking.current = false;
      cancelAnimationFrame(raf);
    };
  }, []);

  return React.useMemo(() => {
    return deriveTopologyActivity(frames, graph, now, life);
  // `now` triggers re-fade; `frames`/`graph` trigger re-derivation.
  }, [frames, graph, life, now]);
}

export function deriveTopologyActivity(
  frames: ConsoleFrame[],
  graph: TopoGraph,
  now: number,
  life = 1500,
): TopoActivity {
  const active: Record<string, number> = {};
  const pulses: TopoPulse[] = [];
  const peerRegistry = new Map<string, string>();
  const busy: Record<string, boolean> = {};
  const calls: Record<string, number> = {};

  const ordered = frames.slice().reverse();
  for (const frame of ordered) {
    const ts = frame.timestampMs || 0;
    if (!ts) continue;

    const identity = frame.identity?.trim();
    if (identity && graph.byId.has(identity)) {
      if ((active[identity] || 0) < ts) active[identity] = ts;
      if (frame.event === "interaction_started" || frame.event === "run_started") {
        busy[identity] = true;
      } else if (
        frame.event === "interaction_complete"
        || frame.event === "interaction_failed"
        || frame.event === "run_completed"
        || frame.event === "run_failed"
        || frame.event === "run_canceled"
      ) {
        busy[identity] = false;
      }
    }

    const data = frameData(frame);
    const name = toolName(data);
    if (name === "peers" && (frame.event === "tool_execution_completed" || frame.event === "tool_result_received")) {
      capturePeerRegistry(peerRegistry, data?.result);
    }

    const typedCommsPulse = typedCommsPulseFromFrame(frame, data, graph);
    if (typedCommsPulse && typedCommsPulse.ts) {
      pulses.push(typedCommsPulse);
      calls[typedCommsPulse.from] = Math.max(calls[typedCommsPulse.from] || 0, typedCommsPulse.ts);
      calls[typedCommsPulse.to] = Math.max(calls[typedCommsPulse.to] || 0, typedCommsPulse.ts);
    }

    const shouldReadPeerToolCalls =
      frame.event === "tool_call_requested"
      || frame.event === "tool_call"
      || frame.event === "tool_execution_started"
      || frame.event === "interaction_complete";
    if (shouldReadPeerToolCalls && identity && graph.byId.has(identity)) {
      for (const call of toolCallsFromFrameData(data).filter((candidate) => PEER_TOOL_NAMES.has(candidate.name))) {
        const args = call.args;
        const recipient = resolvePeerTarget(args, peerRegistry, graph);
        if (recipient && recipient !== identity) {
          pulses.push({
            id: call.id || `${frame.id || ts}-${pulses.length}`,
            from: identity,
            to: recipient,
            ts,
          });
          calls[identity] = Math.max(calls[identity] || 0, ts);
          calls[recipient] = Math.max(calls[recipient] || 0, ts);
        }
      }
    }
  }

  const cutoff = now - life;
  for (const [k, v] of Object.entries(active)) {
    if (v < cutoff) delete active[k];
  }
  for (const [k, v] of Object.entries(calls)) {
    if (v < cutoff) delete calls[k];
  }

  return { active, pulses: pulses.filter((p) => p.ts >= cutoff), busy, calls };
}

/// Convenience: per-edge "is this edge currently carrying a pulse?"
/// derivative. Useful for emphasising hot edges in any layout.
export function edgeKey(a: string, b: string): string {
  return a < b ? `${a}|${b}` : `${b}|${a}`;
}

export function graphStats(graph: TopoGraph): TopoGraphStats {
  const nodeCount = graph.agents.length;
  const edgeCount = graph.edges.length;
  const possibleEdges = nodeCount > 1 ? (nodeCount * (nodeCount - 1)) / 2 : 0;
  const degrees = graph.agents.map((a) => graph.degree[a.id] || 0);
  const minDegree = degrees.length ? Math.min(...degrees) : 0;
  const maxDegree = degrees.length ? Math.max(...degrees) : 0;
  const isolatedCount = degrees.filter((d) => d === 0).length;
  return {
    nodeCount,
    edgeCount,
    possibleEdges,
    density: possibleEdges > 0 ? edgeCount / possibleEdges : 0,
    minDegree,
    maxDegree,
    avgDegree: nodeCount > 0 ? (edgeCount * 2) / nodeCount : 0,
    isolatedCount,
  };
}

export function groupSummaries(graph: TopoGraph): TopoGroupSummary[] {
  const byGroup = new Map<string, TopoGroupSummary>();
  for (const group of graph.groups) {
    byGroup.set(group, { group, count: 0, internalEdges: 0, externalEdges: 0 });
  }
  for (const agent of graph.agents) {
    const summary = byGroup.get(agent.group);
    if (summary) summary.count++;
  }
  for (const edge of graph.edges) {
    const from = graph.byId.get(edge.from);
    const to = graph.byId.get(edge.to);
    if (!from || !to) continue;
    if (from.group === to.group) {
      const summary = byGroup.get(from.group);
      if (summary) summary.internalEdges++;
    } else {
      const a = byGroup.get(from.group);
      const b = byGroup.get(to.group);
      if (a) a.externalEdges++;
      if (b) b.externalEdges++;
    }
  }
  return Array.from(byGroup.values()).sort((a, b) => {
    if (a.count !== b.count) return b.count - a.count;
    return a.group.localeCompare(b.group);
  });
}

export function groupMatrix(graph: TopoGraph, maxGroups = 8): TopoGroupMatrixCell[] {
  const allowed = new Set(groupSummaries(graph).slice(0, maxGroups).map((g) => g.group));
  const keyFor = (group: string) => allowed.has(group) ? group : "Other";
  const counts = new Map<string, number>();
  for (const edge of graph.edges) {
    const from = graph.byId.get(edge.from);
    const to = graph.byId.get(edge.to);
    if (!from || !to) continue;
    const a = keyFor(from.group);
    const b = keyFor(to.group);
    const key = a <= b ? `${a}|${b}` : `${b}|${a}`;
    counts.set(key, (counts.get(key) || 0) + 1);
  }
  return Array.from(counts.entries())
    .map(([key, edges]) => {
      const [from, to] = key.split("|");
      return { from, to, edges };
    })
    .sort((a, b) => {
      if (a.from !== b.from) return a.from.localeCompare(b.from);
      return a.to.localeCompare(b.to);
    });
}

export function sampleEdges(edges: TopoEdge[], limit: number): TopoEdge[] {
  if (edges.length <= limit) return edges;
  if (limit <= 0) return [];
  const step = edges.length / limit;
  const sampled: TopoEdge[] = [];
  let cursor = 0;
  while (sampled.length < limit && Math.floor(cursor) < edges.length) {
    sampled.push(edges[Math.floor(cursor)]);
    cursor += step;
  }
  return sampled;
}
