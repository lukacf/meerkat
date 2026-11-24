// How the console names, places and narrows the mobs a gateway lists: one
// set of functions for every surface (the roster's mob tree, the topology's
// mob choice and team name, Memory's team scopes), so one mob has one name
// and one place everywhere. Pure, with no runtime imports, so every test
// bundle can include it by a relative path.

import { agentDisplayTitle, identityDisplayTitle, isOpaqueIdPart, type AgentTitleSource } from "./agent-title";
import type { ConsoleSidebarStorageLike } from "./sidebar-preferences";

/** One mob as `/console/experience` lists it, at `topology.live_snapshot.mobs`. */
export interface ConsoleMobEntry {
  mob_id: string;
  /** The nearest listed mob it is nested under; absent for a top-level mob. */
  parent_mob_id?: string | null;
  /** The console identity of the member that created it, when the caller may see that member. */
  host_identity?: string | null;
  /**
   * `running`, `stopped`, `dormant` (known and not loaded: asleep),
   * `unavailable` (its store does not load) or `unknown`; any other value
   * reads as unknown.
   */
  state?: string | null;
  /**
   * How many of its members the console lists for the caller. Absent for a
   * mob that is not loaded: nothing is counted for it.
   */
  member_count?: number | null;
  /** How it came to be: `host` for a mob the host created, `member` for a member's; an open list. */
  provenance?: string | null;
  /** True for the mob the gateway was launched with. */
  default?: boolean | null;
  /** An opaque token that changes when a mob is created again under its id. */
  incarnation?: string | null;
}

/** A mob's state as the console reads it. */
export type ConsoleMobState = "running" | "stopped" | "dormant" | "unavailable" | "unknown";

/** The tone of a mob's status mark: one of the kit's status tones. */
export type ConsoleMobTone = "ok" | "warn" | "crit" | "neutral";

/** One mob placed in the console's mob tree. */
export interface ConsoleMobNode {
  mobId: string;
  /**
   * Its name for people ({@link mobDisplayName}): never its id or a piece of
   * it. The id stays a tooltip and what a copy action copies.
   */
  name: string;
  /**
   * Secondary text that tells it apart from another listed mob of the same
   * name ("in Ops team", "2 of 3"); null when its name is its own.
   */
  detail: string | null;
  /** How many mobs it is nested under: 0 at the top level. */
  depth: number;
  /** The mob it is placed under, or null at the top level. */
  parentMobId: string | null;
  hostIdentity: string | null;
  /** The name of the member that created it, when the console knows it. */
  hostName: string | null;
  state: ConsoleMobState;
  /** False for a mob that is listed and not loaded (asleep or unavailable): it has no rows and no count. */
  loaded: boolean;
  memberCount: number;
}

/** The id prefix of a temporary council's mob (Meerkat's `council--<id>`). */
const COUNCIL_PREFIX = "council--";
/** The id prefix of the mob the `delegate` tool runs its helpers in (`implicit-<session>`). */
const IMPLICIT_PREFIX = "implicit-";

/** What a mob is, when its id says: a team, the `delegate` tool's helpers, a council. */
type MobKind = "team" | "helpers" | "council";

/** A mob id as people read it: its kind, and its readable words (lower case; empty for a generated id). */
interface MobIdReading {
  kind: MobKind;
  words: string;
}

/**
 * Whether a piece of a mob id reads as a word: letters ("research"), a word
 * with a short number ("worker2024"), or a short number after a word
 * ("team-1"). Anything else ("a1b2c3", "0193b2a4", "4a5b") is a piece of a
 * generated id.
 */
function isReadableIdPart(part: string, afterWord: boolean): boolean {
  if (/^\p{L}+$/u.test(part)) return true;
  if (/^\p{L}{2,}\d{1,4}$/u.test(part)) return !isOpaqueIdPart(part);
  return afterWord && /^\d{1,4}$/.test(part);
}

/** The readable words an id starts with: "research-0193b2a4-77c1" reads "research", a generated id nothing. */
function readableIdWords(id: string): string {
  const parts = id
    .trim()
    .replace(/([\p{Ll}\p{N}])(\p{Lu})/gu, "$1 $2")
    .split(/[^\p{L}\p{N}]+/u)
    .filter(Boolean);
  const words: string[] = [];
  for (const part of parts) {
    if (!isReadableIdPart(part, words.length > 0)) break;
    words.push(part.toLowerCase());
  }
  return words.join(" ");
}

function readMobId(mobId: string | null | undefined): MobIdReading {
  const id = (mobId ?? "").trim();
  // The `delegate` tool's mob and a council's carry a generated id after
  // their prefix; an id a person wrote that starts the same way is a team's.
  if (id.startsWith(IMPLICIT_PREFIX) && !readableIdWords(id.slice(IMPLICIT_PREFIX.length))) {
    return { kind: "helpers", words: "" };
  }
  if (id.startsWith(COUNCIL_PREFIX) && !readableIdWords(id.slice(COUNCIL_PREFIX.length))) {
    return { kind: "council", words: "" };
  }
  return { kind: "team", words: readableIdWords(id) };
}

/** What the mob is called after a creator's name, lower case: "research team", "team", "helpers". */
function mobNoun(reading: MobIdReading): string {
  if (reading.kind === "helpers") return "helpers";
  if (reading.kind === "council") return "council";
  if (!reading.words) return "team";
  return /\bteam\b/.test(reading.words) ? reading.words : `${reading.words} team`;
}

function sentenceCase(words: string): string {
  return words ? words.charAt(0).toUpperCase() + words.slice(1) : "";
}

/** "Planner's", "Atlas'": a name that ends in s takes the bare apostrophe. */
function possessive(name: string): string {
  return /s$/i.test(name) ? `${name}'` : `${name}'s`;
}

/**
 * A mob's name for people. A mob definition names no other, so the name is
 * read from its id and, when a member created it, from that member's name
 * (`creatorName`):
 * - a readable id reads in words: "ops-team" reads "Ops team", "research"
 *   reads "Research team", "team-1" reads "Team 1"; a readable start is kept
 *   ("research-0193b2a4-77c1" reads "Research team");
 * - a mob a member created reads as that member's: "Planner's research
 *   team"; "Planner's team" for a generated id; "Planner's helpers" for the
 *   mob the `delegate` tool runs its helpers in;
 * - a generated id with no creator named reads "Team" ("Delegated helpers",
 *   "Council" for those kinds).
 * Never the id or a piece of it: the id belongs in a tooltip or a details
 * area. {@link consoleMobTree} tells listed mobs of one name apart.
 */
export function mobDisplayName(mobId: string | null | undefined, creatorName?: string | null): string {
  const reading = readMobId(mobId);
  const creator = (creatorName ?? "").trim();
  if (creator) return `${possessive(creator)} ${mobNoun(reading)}`;
  if (reading.kind === "helpers") return "Delegated helpers";
  return sentenceCase(mobNoun(reading));
}

/** Whether a mob's name says nothing of its own: a generated id and no creator named. */
function isGenericMob(mobId: string, creatorName: string | null): boolean {
  const reading = readMobId(mobId);
  return !creatorName && reading.kind === "team" && !reading.words;
}

/** A listed mob's state, an unknown value as `unknown`. */
export function mobState(state: string | null | undefined): ConsoleMobState {
  return state === "running" || state === "stopped" || state === "dormant" || state === "unavailable" ? state : "unknown";
}

/** Whether a listed mob's state says it is loaded: neither asleep nor unavailable. */
export function mobLoaded(state: string | null | undefined): boolean {
  const read = mobState(state);
  return read !== "dormant" && read !== "unavailable";
}

/** A mob's state in words: "Running", "Stopped", "Asleep", "Unavailable", "Status unknown". */
export function mobStateWords(state: string | null | undefined): string {
  switch (mobState(state)) {
    case "running":
      return "Running";
    case "stopped":
      return "Stopped";
    case "dormant":
      return "Asleep";
    case "unavailable":
      return "Unavailable";
    default:
      return "Status unknown";
  }
}

/**
 * The tone of a mob's status mark, as the topbar marks the console's own
 * mob: a stopped mob red, an unknown one amber. A mob that is asleep is at
 * rest, not at fault: neutral. One whose store does not load is red.
 */
export function mobStateTone(state: string | null | undefined): ConsoleMobTone {
  switch (mobState(state)) {
    case "running":
      return "ok";
    case "stopped":
    case "unavailable":
      return "crit";
    case "dormant":
      return "neutral";
    default:
      return "warn";
  }
}

const MOB_NAME_COLLATOR = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

/** A roster row a mob's creator is named from. */
export type ConsoleMobCreatorRow = AgentTitleSource;

/**
 * Names the member that created a mob: the roster row whose identity is the
 * mob's `host_identity`, by the name every surface gives it; an identity no
 * row names, by its member name in words (`nestedMobs`: the listed mobs
 * other than the console's own).
 */
export function mobCreatorNamer(
  agents: readonly ConsoleMobCreatorRow[] | null | undefined,
  nestedMobs?: ReadonlySet<string> | null,
): (identity: string) => string | null {
  const byKey = new Map<string, ConsoleMobCreatorRow>();
  for (const agent of agents || []) {
    for (const key of [agent.identity, agent.member_id, agent.agent_id]) {
      const trimmed = typeof key === "string" ? key.trim() : "";
      if (trimmed && !byKey.has(trimmed)) byKey.set(trimmed, agent);
    }
  }
  return (identity) => {
    const key = identity.trim();
    if (!key) return null;
    const row = byKey.get(key);
    return (row ? agentDisplayTitle(row) : identityDisplayTitle(key, nestedMobs)) || null;
  };
}

/** How {@link consoleMobTree} names the mobs it places. */
export interface ConsoleMobTreeOptions {
  /** Names a mob's creator from its `host_identity` ({@link mobCreatorNamer}); without it no creator is named. */
  creatorName?: (identity: string) => string | null;
  /**
   * The ordinals given to mobs whose name says nothing of its own ("Team
   * 1", "Team 2"), kept by the caller across snapshots so a mob keeps its
   * ordinal while others come and go. Filled as such mobs are first seen;
   * without it the ordinals follow the ids of the mobs listed now.
   */
  ordinals?: Map<string, number>;
}

/**
 * The listed mobs as a tree, in the order the console lists it: each mob
 * followed by the mobs nested under it, at any depth. At the top level the
 * console's own mob (`primaryMobId`) comes first; siblings are otherwise in
 * name order. A mob whose parent is not listed, or whose parents loop back
 * to it, is placed at the top level. A repeated id is listed once.
 *
 * Every mob gets a name for people ({@link mobDisplayName}). Several mobs
 * whose name says nothing of its own read "Team 1", "Team 2"; other mobs of
 * one name are told apart by their `detail`: the mob each is nested under
 * when that differs, else an ordinal. Never by an id.
 */
export function consoleMobTree(
  mobs: readonly ConsoleMobEntry[] | null | undefined,
  primaryMobId?: string | null,
  options: ConsoleMobTreeOptions = {},
): ConsoleMobNode[] {
  const byId = new Map<string, ConsoleMobEntry>();
  for (const mob of mobs || []) {
    const id = typeof mob?.mob_id === "string" ? mob.mob_id.trim() : "";
    if (id && !byId.has(id)) byId.set(id, mob);
  }
  const listedParent = (id: string): string | null => {
    const parent = byId.get(id)?.parent_mob_id?.trim();
    return parent && parent !== id && byId.has(parent) ? parent : null;
  };
  const onLoop = (id: string): boolean => {
    const seen = new Set<string>();
    let current = listedParent(id);
    while (current && !seen.has(current)) {
      if (current === id) return true;
      seen.add(current);
      current = listedParent(current);
    }
    return false;
  };
  const placedParent = new Map<string, string | null>();
  for (const id of byId.keys()) placedParent.set(id, onLoop(id) ? null : listedParent(id));
  const children = new Map<string | null, string[]>();
  for (const [id, parent] of placedParent) {
    const siblings = children.get(parent) || [];
    siblings.push(id);
    children.set(parent, siblings);
  }
  const primary = primaryMobId?.trim() || null;
  const byIdOrder = (a: string, b: string): number => (a < b ? -1 : a > b ? 1 : 0);

  // Names: the creator's when one is named, the id's words, else "Team".
  const hosts = new Map<string, { identity: string | null; name: string | null }>();
  for (const [id, mob] of byId) {
    const identity = typeof mob.host_identity === "string" && mob.host_identity.trim() ? mob.host_identity.trim() : null;
    hosts.set(id, { identity, name: identity ? options.creatorName?.(identity)?.trim() || null : null });
  }
  const names = new Map<string, string>();
  const generic: string[] = [];
  for (const id of byId.keys()) {
    const creator = hosts.get(id)!.name;
    names.set(id, mobDisplayName(id, creator));
    if (isGenericMob(id, creator)) generic.push(id);
  }
  // A mob that was given an ordinal keeps it while it is listed, alone too.
  const ordinals = options.ordinals ?? new Map<string, number>();
  if (generic.length > 1) {
    let next = Math.max(0, ...ordinals.values()) + 1;
    for (const id of [...generic].sort(byIdOrder)) {
      if (!ordinals.has(id)) ordinals.set(id, next++);
    }
  }
  for (const id of generic) {
    const ordinal = ordinals.get(id);
    if (ordinal !== undefined) names.set(id, `${names.get(id)!} ${ordinal}`);
  }
  // Mobs left with one name: told apart by where each is nested, else by an ordinal.
  const details = new Map<string, string>();
  const sameName = new Map<string, string[]>();
  for (const [id, name] of names) {
    const key = name.toLowerCase();
    sameName.set(key, [...(sameName.get(key) || []), id]);
  }
  for (const ids of sameName.values()) {
    if (ids.length < 2) continue;
    const parents = ids.map((id) => {
      const parent = placedParent.get(id);
      return parent ? names.get(parent)! : "";
    });
    const distinctParents = parents.every(Boolean) && new Set(parents.map((name) => name.toLowerCase())).size === ids.length;
    const ordered = [...ids].sort(byIdOrder);
    ids.forEach((id, index) => {
      details.set(id, distinctParents ? `in ${parents[index]!}` : `${ordered.indexOf(id) + 1} of ${ids.length}`);
    });
  }

  const bySiblingOrder = (a: string, b: string): number => {
    if (a === primary) return -1;
    if (b === primary) return 1;
    const byName = MOB_NAME_COLLATOR.compare(names.get(a)!, names.get(b)!);
    if (byName !== 0) return byName;
    return byIdOrder(a, b);
  };
  const tree: ConsoleMobNode[] = [];
  const visit = (id: string, depth: number): void => {
    const mob = byId.get(id)!;
    const count = Number(mob.member_count);
    const host = hosts.get(id)!;
    tree.push({
      mobId: id,
      name: names.get(id)!,
      detail: details.get(id) ?? null,
      depth,
      parentMobId: placedParent.get(id) ?? null,
      hostIdentity: host.identity,
      hostName: host.name,
      state: mobState(mob.state),
      loaded: mobLoaded(mob.state),
      memberCount: mobLoaded(mob.state) && Number.isFinite(count) && count > 0 ? Math.floor(count) : 0,
    });
    for (const child of [...(children.get(id) || [])].sort(bySiblingOrder)) visit(child, depth + 1);
  };
  for (const root of [...(children.get(null) || [])].sort(bySiblingOrder)) visit(root, 0);
  return tree;
}

/** A mob's name with what tells it apart, in one line: "Research team (2 of 3)". */
export function mobFullName(mob: Pick<ConsoleMobNode, "name" | "detail">): string {
  return mob.detail ? `${mob.name} (${mob.detail})` : mob.name;
}

/** `mobId` and every mob nested under it in `tree`; empty when the tree does not list it. */
export function mobAndNestedIds(tree: readonly ConsoleMobNode[], mobId: string | null | undefined): Set<string> {
  const ids = new Set<string>();
  const root = (mobId ?? "").trim();
  if (!root || !tree.some((mob) => mob.mobId === root)) return ids;
  ids.add(root);
  let grew = true;
  while (grew) {
    grew = false;
    for (const mob of tree) {
      if (mob.parentMobId && ids.has(mob.parentMobId) && !ids.has(mob.mobId)) {
        ids.add(mob.mobId);
        grew = true;
      }
    }
  }
  return ids;
}

/** The field a member's mob is read from. */
export interface ConsoleMobMemberSource {
  mob_id?: string | null;
}

/** The mob an agent row belongs to: its `mob_id`, else the console's own mob. */
export function agentMobId(agent: ConsoleMobMemberSource, primaryMobId?: string | null): string | undefined {
  const fromRow = typeof agent.mob_id === "string" ? agent.mob_id.trim() : "";
  return fromRow || primaryMobId?.trim() || undefined;
}

/**
 * The listed mob a row is shown in: its own mob when the console lists it
 * (`listed`), else the console's own mob, else the first mob listed. One
 * rule for the roster's grouping, the mob choice and the topology, so a row
 * that names a mob the console does not list is shown in one place under
 * every view and is never lost to a mob choice.
 */
export function listedMobOf(
  agent: ConsoleMobMemberSource,
  listed: ReadonlySet<string>,
  primaryMobId?: string | null,
): string | undefined {
  const own = agentMobId(agent, primaryMobId);
  if (own && listed.has(own)) return own;
  const primary = primaryMobId?.trim();
  if (primary && listed.has(primary)) return primary;
  return listed.values().next().value ?? own;
}

/** An agent of the topology filter: a mob member and the keys a node may name it by. */
export interface ConsoleMobTopologyAgent extends ConsoleMobMemberSource {
  identity?: string | null;
  member_id?: string | null;
  agent_id?: string | null;
}

/** A topology node of the filter: an identity or endpoint key, the identity its endpoint names, and its mob when the row says. */
export interface ConsoleMobTopologyNode {
  identity?: string | null;
  mob_id?: string | null;
  ref?: { identity?: string | null } | null;
}

/**
 * The topology of the chosen mobs (`mobIds`; null for all mobs): the agents
 * of those mobs, and the nodes whose member is in one of them. A node names
 * its mob itself, or through the agent its identity (or its endpoint's
 * identity) names; a node of neither is the console's own mob's. A mob the
 * console does not list (`listedMobIds`) reads as {@link listedMobOf} reads
 * it. With all mobs the lists are returned as they are.
 */
export function topologyInMobs<N extends ConsoleMobTopologyNode, A extends ConsoleMobTopologyAgent>(args: {
  nodes: N[];
  agents: A[];
  mobIds: ReadonlySet<string> | null;
  primaryMobId?: string | null;
  listedMobIds?: ReadonlySet<string> | null;
}): { nodes: N[]; agents: A[] } {
  const { nodes, agents, mobIds, primaryMobId, listedMobIds } = args;
  if (!mobIds) return { nodes, agents };
  const mobOf = (source: ConsoleMobMemberSource): string | undefined =>
    (listedMobIds ? listedMobOf(source, listedMobIds, primaryMobId) : agentMobId(source, primaryMobId));
  const agentByKey = new Map<string, A>();
  for (const agent of agents) {
    for (const key of [agent.identity, agent.member_id, agent.agent_id]) {
      const trimmed = typeof key === "string" ? key.trim() : "";
      if (trimmed && !agentByKey.has(trimmed)) agentByKey.set(trimmed, agent);
    }
  }
  const nodeMob = (node: N): string | undefined => {
    if (typeof node.mob_id === "string" && node.mob_id.trim()) return mobOf(node);
    for (const key of [node.ref?.identity, node.identity]) {
      const agent = typeof key === "string" ? agentByKey.get(key.trim()) : undefined;
      if (agent) return mobOf(agent);
    }
    return mobOf({});
  };
  return {
    agents: agents.filter((agent) => {
      const mob = mobOf(agent);
      return mob !== undefined && mobIds.has(mob);
    }),
    nodes: nodes.filter((node) => {
      const mob = nodeMob(node);
      return mob !== undefined && mobIds.has(mob);
    }),
  };
}

/** The mob choice in effect, read from the stored choice and the mobs listed now. */
export interface ConsoleMobChoice {
  /** The chosen mob, or null for all mobs. */
  mobId: string | null;
  /** The chosen mob and the mobs nested under it, or null for all mobs. */
  mobIds: Set<string> | null;
  /**
   * The stored choice names a mob that a loaded mob list does not hold: the
   * caller writes "all mobs" over it, so an id a later mob reuses does not
   * bring the choice back.
   */
  drop: boolean;
}

/**
 * The mob choice in effect. A stored mob applies while the console lists it
 * among several mobs; with one mob listed there is nothing to choose, and
 * the choice rests without being dropped. A stored mob that a loaded list
 * (`listLoaded`) does not hold is to be dropped from storage.
 */
export function resolveConsoleMobChoice(
  tree: readonly ConsoleMobNode[],
  stored: string | null | undefined,
  listLoaded: boolean,
): ConsoleMobChoice {
  const mobId = (stored ?? "").trim();
  if (!mobId) return { mobId: null, mobIds: null, drop: false };
  const ids = mobAndNestedIds(tree, mobId);
  if (ids.size === 0) return { mobId: null, mobIds: null, drop: listLoaded };
  if (tree.length < 2) return { mobId: null, mobIds: null, drop: false };
  return { mobId, mobIds: ids, drop: false };
}

/**
 * The mob choice the console keeps per user, beside the roster view: one
 * mob (and the mobs nested under it) or all mobs. Its key takes the
 * principal-aware preference scope.
 */
export const MOB_VIEW_STORAGE_PREFIX = "meta-console-mob-view";

/** The stored mob choice: a mob id, or null for all mobs (also when nothing readable is stored). */
export function readConsoleMobView(storage: ConsoleSidebarStorageLike | null | undefined, key: string): string | null {
  if (!storage) return null;
  try {
    const raw = storage.getItem(key);
    if (raw === null) return null;
    const parsed: unknown = JSON.parse(raw);
    const record = parsed && typeof parsed === "object" ? parsed as Record<string, unknown> : {};
    const mobId = typeof record.mob_id === "string" ? record.mob_id.trim() : "";
    return mobId || null;
  } catch {
    return null;
  }
}

/** Keep the mob choice: a mob id, or null for all mobs. */
export function writeConsoleMobView(
  storage: ConsoleSidebarStorageLike | null | undefined,
  key: string,
  mobId: string | null,
): void {
  if (!storage) return;
  try {
    storage.setItem(key, JSON.stringify({ v: 1, mob_id: mobId?.trim() || null }));
  } catch {
    /* ignore */
  }
}
