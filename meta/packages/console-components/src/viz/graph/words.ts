// Words for the graph: relation and kind names, the graph's summary and a
// node's accessible name (people first, machines on request: names, roles,
// states and counts, never ids).

import type { VizEdge, VizNode, VizNodeKind, VizRelation } from "./types";

const RELATION_WORDS: Record<VizRelation, [string, string]> = {
  wired: ["wired", "wired"],
  traffic: ["messages", "messages"],
  route: ["route", "routes"],
  lineage: ["helper", "helpers"],
  cross: ["other team", "other teams"],
  blocks: ["blocks", "blocks"],
  parent: ["also under", "also under"],
  related: ["related", "related"],
};

const KIND_WORDS: Record<VizNodeKind, [string, string]> = {
  agent: ["agent", "agents"],
  coordinator: ["coordinator", "coordinators"],
  helper: ["helper", "helpers"],
  person: ["person", "people"],
  source: ["channel", "channels"],
  store: ["memory store", "memory stores"],
  external: ["other team", "other teams"],
  unknown: ["unobserved", "unobserved"],
  item: ["item", "items"],
  goal: ["goal", "goals"],
};

/** A relation in words ("wired", "messages", "helper"); plural when `count` is not 1. */
export function vizRelationWord(relation: VizRelation, count = 1): string {
  const words = RELATION_WORDS[relation] ?? [relation, relation];
  return count === 1 ? words[0] : words[1];
}

/** A node kind in words ("agent", "channel", "memory store"); plural when `count` is not 1. */
export function vizKindWord(kind: VizNodeKind, count = 1): string {
  const words = KIND_WORDS[kind] ?? [kind, `${kind}s`];
  return count === 1 ? words[0] : words[1];
}

/** "in_progress" -> "In progress". */
export function humanizeWord(word: string): string {
  const text = word.replace(/[_-]+/g, " ").trim();
  return text ? text.charAt(0).toUpperCase() + text.slice(1).toLowerCase() : "";
}

/** The status a person reads: the given label, else the raw word humanized. */
export function nodeStatusWord(node: Pick<VizNode, "status" | "statusLabel" | "live" | "retired">): string {
  if (node.statusLabel) return node.statusLabel;
  if (node.status) return humanizeWord(node.status);
  if (node.retired) return "Retired";
  if (node.live) return "Working";
  return "";
}

const AGENT_KINDS: ReadonlySet<VizNodeKind> = new Set(["agent", "coordinator", "helper", "external", "unknown"]);

/**
 * The graph's summary, its accessible name: "7 agents, 6 connections".
 * Agents of every kind count as agents; items and goals as items with links.
 */
export function graphSummary(nodes: readonly VizNode[], edges: readonly VizEdge[]): string {
  const work = nodes.filter((node) => node.kind === "item" || node.kind === "goal").length;
  const agents = nodes.filter((node) => AGENT_KINDS.has(node.kind)).length;
  const parts: string[] = [];
  if (agents > 0) parts.push(`${agents} ${agents === 1 ? "agent" : "agents"}`);
  if (work > 0) parts.push(`${work} ${work === 1 ? "item" : "items"}`);
  const others = new Map<VizNodeKind, number>();
  for (const node of nodes) {
    if (AGENT_KINDS.has(node.kind) || node.kind === "item" || node.kind === "goal") continue;
    others.set(node.kind, (others.get(node.kind) ?? 0) + 1);
  }
  for (const [kind, count] of others) parts.push(`${count} ${vizKindWord(kind, count)}`);
  if (parts.length === 0) parts.push("0 nodes");
  const linkWord = work > 0 && agents === 0 ? ["link", "links"] : ["connection", "connections"];
  parts.push(`${edges.length} ${edges.length === 1 ? linkWord[0] : linkWord[1]}`);
  return parts.join(", ");
}

/** "Billing, Lead · Domain, Idle, 3 connections": what the live region and the node's name say. */
export function nodeAccessibleName(node: VizNode, degree: number): string {
  if (node.ariaLabel) return node.ariaLabel;
  const parts = [node.label];
  if (node.sublabel) parts.push(node.sublabel);
  const status = nodeStatusWord(node);
  if (status) parts.push(status);
  if (node.kind !== "item" && node.kind !== "goal") parts.push(`${degree} ${degree === 1 ? "connection" : "connections"}`);
  return parts.join(", ");
}

/** The tooltip lines for a node: its title, else label, sublabel and status. */
export function nodeTooltipLines(node: VizNode): string[] {
  if (node.title) return node.title.split("\n").filter(Boolean);
  const lines = [node.label];
  if (node.sublabel) lines.push(node.sublabel);
  const status = nodeStatusWord(node);
  if (status) lines.push(status);
  return lines;
}

/** The tooltip lines for an edge: its title, else "A to B · relation · label". */
export function edgeTooltipLines(edge: VizEdge, from?: VizNode, to?: VizNode): string[] {
  if (edge.title) return edge.title.split("\n").filter(Boolean);
  const ends = `${from?.label ?? edge.from} to ${to?.label ?? edge.to}`;
  const parts = [ends, vizRelationWord(edge.relation)];
  if (edge.label) parts.push(edge.label);
  if (edge.state === "pending") parts.push("Connecting");
  if (edge.state === "failed") parts.push("Deliveries failed");
  if (edge.state === "declared") parts.push("Declared, not connected");
  return [parts.join(" · ")];
}
