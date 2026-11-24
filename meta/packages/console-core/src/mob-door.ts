// The console's one owner of how a member is addressed when a gateway hosts
// more than one mob.
//
// A gateway serves the mob it was launched with (the launch mob) and may
// host others, each behind a door of its own: a request for a member of a
// hosted mob names that mob (`mob_id`) and the member as that mob knows it
// (`identity`). A mob a member created is served through the door of the
// mob its tree starts at.
//
// The console keeps one key per member, the row key, in every map, storage
// key and dock target: the bare name for a member of the launch mob, and
// `<member's own mob id>/<name>` for any other member (the form of
// ./member-ref). The key is presentation and a browser-side key only. Two
// functions cross between the key and the wire:
//
// - {@link memberDoor}: the request pair for a row key;
// - {@link memberRowKey}: the row key for what a frame or an answer names.
//
// A gateway that does not list the `mob_addressing` capability is sent no
// `mob_id` and no `mobs`: every request is the request a one-mob console
// sends. No other console code composes the pair or reads a frame's mob.
// The words on the wire are the contract's (./contract, v0.6.0), and the
// two functions are pinned by the shared cases the gateway's are pinned by
// (console_row_key_cases.json). Pure, with no runtime imports, so every
// test bundle can include it by a relative path.

import {
  CONSOLE_MOB_ADDRESSING_CAPABILITY,
  CONSOLE_MOBS_SELECTOR_EVERY,
  CONSOLE_MOBS_SELECTOR_PARAM,
} from "./contract";
import { nestedMemberIdentity, parseNestedMemberIdentity } from "./member-ref";
import { mobLoaded } from "./mobs";

// A person's opening of a mob sends the contract's opening request
// (`CONSOLE_MOB_OPEN_METHOD`: an ordinary read of the mob's status, naming
// the mob). Serving it loads a mob that is asleep; there is no word on the
// wire for "load".

/**
 * A request the console makes on its own was held back: its member's mob is
 * listed as not loaded, and only a person's act names such a mob. Nothing
 * was sent.
 */
export class ConsoleMobNotLoadedError extends Error {
  readonly kind = "console-mob-not-loaded";
  constructor(readonly identity: string) {
    super("This agent's team is not loaded. Nothing was asked of it.");
    this.name = "ConsoleMobNotLoadedError";
  }
}

/**
 * A command named a member of a mob with a door of its own, and the
 * transport does not know how the command's method names a member. Sent
 * as it is, it would be answered for a member of the launch mob that has
 * the same name. Nothing was sent.
 */
export class ConsoleMemberDoorUnknownError extends Error {
  readonly kind = "console-member-door-unknown";
  constructor(readonly identity: string, readonly method: string) {
    super("This action is not available yet for an agent of this team. Nothing was sent.");
    this.name = "ConsoleMemberDoorUnknownError";
  }
}

/** Whether an error is {@link ConsoleMobNotLoadedError}: a request of the console's own that was held back. */
export function isConsoleMobNotLoaded(error: unknown): boolean {
  return (error as { kind?: unknown } | null)?.kind === "console-mob-not-loaded";
}

/**
 * Who asks for a request. A request the console makes on its own (a pane's
 * read when it opens, after a frame, on a timer or a retry) is automatic:
 * it is never made for a member whose mob is listed as not loaded. A
 * person's act says nothing and is sent.
 */
export interface ConsoleRequestBy {
  automatic?: boolean;
  /**
   * The row the person acted on, by the console's key for it. A command
   * sent for a row goes to that row's door, whatever name its params
   * carry.
   */
  about?: string;
  /**
   * The command acts on a value of the launch mob's door that names no
   * row (the docked work graph): it goes to that door, and is not sent
   * when its params name a member behind another door.
   */
  launchDoor?: boolean;
}

/** The console asks on its own. */
export const AUTOMATIC: ConsoleRequestBy = Object.freeze({ automatic: true });

/** What a listed mob says of its place and its door (`topology.live_snapshot.mobs[]`). */
export interface MobDoorSource {
  mob_id?: string | null;
  parent_mob_id?: string | null;
  /** How the mob came to be: `host` for a mob the host created, `member` for a member's; other words read as unknown. */
  provenance?: string | null;
  /** True for the launch mob. */
  default?: boolean | null;
  state?: string | null;
  /** An opaque token that changes when a mob is created again under its id. */
  incarnation?: string | null;
}

/** The doors of the mobs a console lists. Built by {@link mobDoorTable}. */
export interface MobDoorTable {
  /** Whether the gateway judges a request's mob. Without it nothing is addressed or keyed by a mob. */
  readonly addressing: boolean;
  /** The launch mob's id, when the console knows it. */
  readonly launchMobId: string | null;
  /** Each listed mob's door: the hosted mob its tree starts at, or null for the launch mob's door. */
  readonly doors: ReadonlyMap<string, string | null>;
  /** The listed mobs that are not loaded: asleep, or unavailable. */
  readonly notLoaded: ReadonlySet<string>;
}

/** A table with no door: every request is the request of a one-mob console. */
export const NO_MOB_DOORS: MobDoorTable = Object.freeze({
  addressing: false,
  launchMobId: null,
  doors: new Map<string, string | null>(),
  notLoaded: new Set<string>(),
});

const text = (value: unknown): string => (typeof value === "string" ? value.trim() : "");

/**
 * The doors of the listed mobs. A mob has a door of its own when the host
 * created it (`provenance` is `host`), it is placed at the top level and it
 * is not the launch mob; every mob nested under it shares that door. Every
 * other mob is served through the launch mob's door, as before.
 *
 * `addressing` is whether the gateway lists {@link CONSOLE_MOB_ADDRESSING_CAPABILITY}:
 * without it no mob has a door of its own.
 */
export function mobDoorTable(
  mobs: ReadonlyArray<MobDoorSource> | null | undefined,
  primaryMobId: string | null | undefined,
  addressing: boolean,
  /**
   * The mobs seen behind a hosted mob's door before, each with that door. A
   * mob a member created is listed only while the mob it is nested under is
   * loaded: while that hosted mob is listed and not loaded, such a mob
   * keeps the door it had and counts as not loaded too.
   */
  behind?: ReadonlyMap<string, string> | null,
): MobDoorTable {
  const byId = new Map<string, MobDoorSource>();
  for (const mob of mobs || []) {
    const id = text(mob?.mob_id);
    if (id && !byId.has(id)) byId.set(id, mob);
  }
  let launch = text(primaryMobId) || null;
  for (const [id, mob] of byId) {
    if (mob.default === true) {
      launch = id;
      break;
    }
  }
  const listedParent = (id: string): string | null => {
    const parent = text(byId.get(id)?.parent_mob_id);
    return parent && parent !== id && byId.has(parent) ? parent : null;
  };
  // The mob a tree starts at: a mob whose parents loop back to it starts its own.
  const rootOf = (id: string): string => {
    const seen = new Set<string>([id]);
    let current = id;
    for (let parent = listedParent(current); parent; parent = listedParent(current)) {
      if (seen.has(parent)) return id;
      seen.add(parent);
      current = parent;
    }
    return current;
  };
  const hasOwnDoor = (id: string): boolean =>
    addressing && id !== launch && byId.get(id)?.default !== true && text(byId.get(id)?.provenance) === "host";
  const doors = new Map<string, string | null>();
  const notLoaded = new Set<string>();
  for (const [id, mob] of byId) {
    const root = rootOf(id);
    doors.set(id, hasOwnDoor(root) ? root : null);
    if (!mobLoaded(mob.state)) notLoaded.add(id);
  }
  if (addressing && behind) {
    for (const [id, door] of behind) {
      if (byId.has(id) || doors.get(door) !== door || !notLoaded.has(door)) continue;
      doors.set(id, door);
      notLoaded.add(id);
    }
  }
  return { addressing, launchMobId: launch, doors, notLoaded };
}

/** How a request addresses a member. */
export interface MemberDoor {
  /** The mob the request names (`mob_id`), or null: the request names no mob. */
  mobId: string | null;
  /** The member as that door knows it (`identity`). */
  identity: string;
}

/**
 * A member as its row names it (contract v0.6.0,
 * `mob_addressing.member_rows`): the door the row was read on, the mob the
 * member sits in and its bare name there.
 */
export interface MemberOfRow {
  /** The hosted mob whose door the row was read on, or null for the launch mob's door. */
  door: string | null;
  /** The mob the member sits in (`member_mob_id`). */
  memberMobId: string;
  /** The member's bare name in that mob (`member_name`). */
  memberName: string;
  /** The member as its door knows it: the bare name in the door's own mob, `<mob id>/<name>` in a mob under it. */
  atDoor: string;
}

/**
 * The members the roster's rows name, by row key. A key two rows spell for
 * two members is held as null: no reading of the key tells the two apart,
 * so nothing is addressed by it.
 */
export type MemberRows = ReadonlyMap<string, MemberOfRow | null>;

const NO_MEMBER_ROWS: MemberRows = new Map<string, MemberOfRow | null>();

/**
 * The members an experience document's rows name
 * (`agent_sidebar.live_snapshot.agents[]`), by row key. A row is read when
 * it carries the mob its member sits in and the member's bare name there,
 * and the door it names is the launch mob's or a listed mob with a door of
 * its own; any other row is left to the reading of its key. Without mob
 * addressing no row is read.
 */
export function memberRowsOf(experience: unknown, table: MobDoorTable): Map<string, MemberOfRow | null> {
  const rows = new Map<string, MemberOfRow | null>();
  if (!table.addressing) return rows;
  const listed = record(record(record(experience)?.agent_sidebar)?.live_snapshot)?.agents;
  if (!Array.isArray(listed)) return rows;
  for (const entry of listed) {
    const row = record(entry);
    const key = text(row?.identity);
    const memberMobId = text(row?.member_mob_id);
    const memberName = text(row?.member_name);
    if (!row || !key || !memberMobId || !memberName) continue;
    const named = text(row.mob_id);
    const door = !named || named === table.launchMobId ? null : named;
    if (door !== null && table.doors.get(door) !== door) continue;
    // The mob of the row's door: the hosted mob, or the launch mob by the id the row or the list gives it.
    const doorMob = door ?? (named || table.launchMobId);
    if (!doorMob) continue;
    const atDoor = memberMobId === doorMob ? memberName : nestedMemberIdentity(memberMobId, memberName);
    if (!atDoor) continue;
    const member: MemberOfRow = { door, memberMobId, memberName, atDoor };
    const held = rows.get(key);
    if (held === undefined) rows.set(key, member);
    else if (held === null || held.door !== door || held.memberMobId !== memberMobId || held.memberName !== memberName) rows.set(key, null);
  }
  return rows;
}

/**
 * The request pair for a member's row key (contract v0.6.0,
 * `mob_addressing.row_key`). The door and the member's name there are the
 * ones the member's row names (`rows`): its door, and the member's bare
 * name in the door's own mob or `<mob id>/<name>` in a mob under it. A key
 * with no row is read by its spelling: a key whose first part is a hosted
 * mob with a door of its own names that door and, after the separator, the
 * member as that door knows it; every other key is addressed with no mob,
 * and the key.
 *
 * A key `<nested mob id>/<name>` for a member of a mob nested under a
 * hosted mob is read too: the hosted mob's door and the key.
 */
export function memberDoor(rowKey: string, table: MobDoorTable, rows: MemberRows = NO_MEMBER_ROWS): MemberDoor {
  const row = rows.get(rowKey);
  if (row) return { mobId: row.door, identity: row.atDoor };
  const parsed = parseNestedMemberIdentity(rowKey);
  const door = parsed ? table.doors.get(parsed.mobId) : undefined;
  if (!parsed || !door) return { mobId: null, identity: rowKey };
  return door === parsed.mobId ? { mobId: door, identity: parsed.name } : { mobId: door, identity: rowKey };
}

/**
 * The mob a row key's member belongs to, when the console lists it; else
 * null (the launch mob's member). The member's row says it (`rows`); a key
 * with no row is read by its spelling.
 */
export function memberMobId(rowKey: string, table: MobDoorTable, rows: MemberRows = NO_MEMBER_ROWS): string | null {
  const row = rows.get(rowKey);
  if (row) return row.memberMobId === table.launchMobId || !table.doors.has(row.memberMobId) ? null : row.memberMobId;
  const parsed = parseNestedMemberIdentity(rowKey);
  if (!parsed || !table.doors.has(parsed.mobId)) return null;
  if (table.doors.get(parsed.mobId) !== parsed.mobId) return parsed.mobId;
  // Behind a hosted mob's door: the mob nested under it that the rest names, else the hosted mob.
  const nested = parseNestedMemberIdentity(parsed.name);
  return nested && nested.mobId !== parsed.mobId && table.doors.get(nested.mobId) === parsed.mobId
    ? nested.mobId
    : parsed.mobId;
}

/** What a frame, or an answer of a door, says of the member it is about. */
export interface MemberOnTheWire {
  /** The mob it names; absent for the launch mob. */
  mob_id?: string | null;
  identity?: string | null;
}

/**
 * The name of a member of the listed mob `mobId` as the mob's door knows
 * it: the bare name in the door's own mob, `<mob id>/<name>` in a mob
 * nested under the door.
 */
function nameAtDoor(mobId: string, door: string | null, name: string): string | null {
  return mobId === door ? name : nestedMemberIdentity(mobId, name);
}

/**
 * The row key of a member by its own mob and its name (contract v0.6.0,
 * `mob_addressing.row_key`): the member as its door knows it, and for a
 * door other than the launch mob's that door's id and the separator before
 * it. So the bare name for a member of the launch mob,
 * `<mob id>/<name>` for a member of a mob nested under the launch mob or of
 * a hosted mob, and `<hosted mob id>/<mob id>/<name>` for a member of a mob
 * nested under a hosted mob. Null for an empty name and for a mob the list
 * does not hold: such a member has no row.
 */
export function memberRowKeyOf(mobId: string, name: string, table: MobDoorTable): string | null {
  if (!name || !table.doors.has(mobId)) return null;
  if (mobId === table.launchMobId) return name;
  const door = table.doors.get(mobId) ?? null;
  const atDoor = nameAtDoor(mobId, door, name);
  if (atDoor === null) return null;
  return door === null ? atDoor : nestedMemberIdentity(door, atDoor);
}

/**
 * The row key of the member a frame names, or null for a frame that
 * belongs to no row (contract v0.6.0, `mob_addressing.row_key`). A frame
 * names the door it was served by (`mob_id`, absent for the launch mob's)
 * and the member as that door knows it. A frame of the launch mob's door
 * keeps its identity: the key of today. A frame of a hosted mob's door is
 * keyed by that mob, the separator and its identity, whatever the identity
 * holds (a member of a mob nested under the hosted mob is
 * `<nested mob id>/<name>` at that door). A frame that names a listed mob
 * with no door of its own is keyed as a member of that mob. A frame with no
 * identity, and a frame of a mob the list does not hold, belong to no row:
 * never to a member of the launch mob that has the same name. Without mob
 * addressing a frame keeps its identity.
 */
export function memberRowKey(member: MemberOnTheWire, table: MobDoorTable): string | null {
  const identity = text(member.identity);
  const mob = text(member.mob_id);
  if (!table.addressing) return identity;
  if (!identity) return null;
  if (!mob || mob === table.launchMobId) return identity;
  if (!table.doors.has(mob)) return null;
  const door = table.doors.get(mob) ?? null;
  if (door === mob) return nestedMemberIdentity(mob, identity) ?? identity;
  // A frame that names a mob nested under a door: its identity may already
  // be the member as the door knows it (`<mob id>/<name>`), else its name.
  const nested = parseNestedMemberIdentity(identity);
  const name = nested && nested.mobId === mob ? nested.name : identity;
  return memberRowKeyOf(mob, name, table) ?? identity;
}

/** Whether a row key's member is of a listed mob that is not loaded, or is served through the door of one. */
export function memberMobNotLoaded(rowKey: string, table: MobDoorTable, rows: MemberRows = NO_MEMBER_ROWS): boolean {
  const mob = memberMobId(rowKey, table, rows);
  if (!mob) return false;
  const door = table.doors.get(mob);
  return table.notLoaded.has(mob) || (door ? table.notLoaded.has(door) : false);
}

/** A frame as the transport reads it off the wire: its member, and the mob it names beside it. */
export interface FrameOnTheWire {
  event?: string;
  identity?: string;
  mobId?: string;
  data?: unknown;
}

const record = (value: unknown): Record<string, unknown> | null =>
  value && typeof value === "object" ? (value as Record<string, unknown>) : null;

/**
 * The mob a frame names when the list holds no such mob (a mob created a
 * moment ago, or any mob with a door of its own before the list of every
 * mob is read); else null. Nothing of such a frame has a row.
 */
export function frameUnlistedMob(frame: FrameOnTheWire, table: MobDoorTable, doorMobId?: string | null): string | null {
  if (!table.addressing) return null;
  const mob = frame.mobId || doorMobId || "";
  return mob && mob !== table.launchMobId && !table.doors.has(mob) ? mob : null;
}

/**
 * A frame keyed by its member's row ({@link memberRowKey}), handed on
 * without the mob it named: the key says it. `doorMobId` is the mob of the
 * door the frame was read through, for a frame that names none itself. A
 * `frame_updated` frame carries the frame it updates, which is keyed the
 * same way. Null for a frame of a mob the list does not hold
 * ({@link frameUnlistedMob}): it is not handed on, neither under a launch
 * member's key nor under one made of the mob's id. Without mob addressing
 * the frame is returned as it is.
 */
export function frameUnderRowKey<F extends FrameOnTheWire>(frame: F, table: MobDoorTable, doorMobId?: string | null): F | null {
  if (!table.addressing) return frame;
  if (frameUnlistedMob(frame, table, doorMobId)) return null;
  const mob = frame.mobId || doorMobId || undefined;
  let next = frame;
  if (frame.mobId !== undefined) {
    const { mobId: _named, ...rest } = frame;
    next = rest as F;
  }
  if (frame.identity && mob) {
    const key = memberRowKey({ mob_id: mob, identity: frame.identity }, table) ?? frame.identity;
    if (key !== frame.identity) next = { ...next, identity: key };
  }
  const data = frame.event === "frame_updated" ? record(frame.data) : null;
  const inner = data ? record(data.frame) : null;
  if (data && inner) {
    const keyed = frameUnderRowKey(inner as FrameOnTheWire, table, mob);
    if (keyed === null) return null;
    if (keyed !== inner) next = { ...next, data: { ...data, frame: keyed } };
  }
  return next;
}

/** The members an answer of a door may name at its top level, each as that door knows it. */
const ANSWER_MEMBER_FIELDS = ["identity", "agent_identity", "member_id", "side_chat_of", "source_member_id"] as const;

/**
 * An answer of a door other than the launch mob's, handed on under the
 * console's keys: the members it names at its top level (its own member, a
 * fork's new member and its source) are keyed as the roster keys them
 * ({@link memberRowKey}). Anything else is left as it came: an answer that
 * is no object, a member that is not a string, a name with no row.
 */
export function answerUnderRowKeys<T>(answer: T, doorMobId: string, table: MobDoorTable, memberWord?: string | null): T {
  const fields = Array.isArray(answer) ? null : record(answer);
  if (!fields) return answer;
  let keyed: Record<string, unknown> | null = null;
  // The word the request named its member by names it in the answer too.
  for (const field of memberWord && !ANSWER_MEMBER_FIELDS.includes(memberWord as never) ? [...ANSWER_MEMBER_FIELDS, memberWord] : ANSWER_MEMBER_FIELDS) {
    const named = fields[field];
    if (typeof named !== "string" || !named) continue;
    const key = memberRowKey({ mob_id: doorMobId, identity: named }, table);
    if (key === null || key === named) continue;
    keyed ??= { ...fields };
    keyed[field] = key;
  }
  // An inspect answer names its member one level down: `identity.identity`.
  const owner = record(fields.identity);
  const ownerName = owner && !Array.isArray(fields.identity) ? owner.identity : undefined;
  if (owner && typeof ownerName === "string" && ownerName) {
    const key = memberRowKey({ mob_id: doorMobId, identity: ownerName }, table);
    if (key !== null && key !== ownerName) {
      keyed ??= { ...fields };
      keyed.identity = { ...owner, identity: key };
    }
  }
  // A schedule names its agent as the door knows it: each schedule of a
  // list, and a schedule an answer carries by itself, is kept under the
  // console's key for that agent. (Who a schedule sends as may be a person:
  // it is left as the door wrote it, and read where it is shown.)
  const underRowKey = (schedule: unknown): unknown => {
    const item = record(schedule);
    const named = item && !Array.isArray(schedule) ? item.identity : undefined;
    if (!item || typeof named !== "string" || !named) return schedule;
    const key = memberRowKey({ mob_id: doorMobId, identity: named }, table);
    return key === null || key === named ? schedule : { ...item, identity: key };
  };
  if (Array.isArray(fields.schedules)) {
    const schedules = fields.schedules.map(underRowKey);
    if (schedules.some((schedule, index) => schedule !== (fields.schedules as unknown[])[index])) {
      keyed ??= { ...fields };
      keyed.schedules = schedules;
    }
  }
  if (fields.schedule !== undefined) {
    const schedule = underRowKey(fields.schedule);
    if (schedule !== fields.schedule) {
      keyed ??= { ...fields };
      keyed.schedule = schedule;
    }
  }
  // An inspect answer names its member's peers as the door knows them:
  // `peers` beside its `identity` object, or `topology_peers`. Each is a
  // member of that door's mob, never a launch member of the same name. (An
  // agent's record also has `peers`, and no `identity` object: it is left
  // as the door wrote it, for it goes back to the same door.)
  for (const list of owner && !Array.isArray(fields.identity) ? ["peers", "topology_peers"] : ["topology_peers"]) {
    const named = fields[list];
    if (!Array.isArray(named)) continue;
    const under = named.map((peer) =>
      (typeof peer === "string" && peer ? memberRowKey({ mob_id: doorMobId, identity: peer }, table) ?? peer : peer));
    if (under.some((peer, index) => peer !== named[index])) {
      keyed ??= { ...fields };
      keyed[list] = under;
    }
  }
  return (keyed ?? answer) as T;
}

/**
 * Whether a bare name in one of the host's lists could be an agent of more
 * than one mob. The host's lists (the questions agents asked, the
 * approvals, the person's replies) name an agent by its name and no mob.
 * Such a name is ambiguous where a listed member behind a door of its own
 * is known to its door by that name, and where a mob with a door of its
 * own is not loaded: its members are not known, so the name could be one
 * of theirs. `rowKeys` are the keys of the roster's rows. Without mob
 * addressing no name is ambiguous.
 */
export function hostListNameAmbiguous(name: string, table: MobDoorTable, rowKeys: Iterable<string>): boolean {
  if (!table.addressing || !name) return false;
  for (const [mob, door] of table.doors) {
    if (door === mob && table.notLoaded.has(mob)) return true;
  }
  for (const key of rowKeys) {
    const door = memberDoor(key, table);
    if (door.mobId !== null && door.identity === name) return true;
  }
  return false;
}

/**
 * How an ambiguous name of the host's lists is handed on: words that say
 * what the entry says and what it does not, and that are no agent's key,
 * so the entry is placed in no chat and opens no agent.
 */
export function hostListAgentWords(name: string): string {
  return `${name}${HOST_LIST_WORDS_ENDING}`;
}

const HOST_LIST_WORDS_ENDING = " (team not named)";

/**
 * The words as they are shown, where a key is one {@link hostListAgentWords}
 * made; undefined for any other key. A reader that writes agent names asks
 * this first, so the words are never written, or opened, as an agent.
 */
export function hostListAgentWordsOf(key: string): string | undefined {
  const trimmed = key.trim();
  return trimmed.length > HOST_LIST_WORDS_ENDING.length && trimmed.endsWith(HOST_LIST_WORDS_ENDING) ? trimmed : undefined;
}

/** The mobs an experience lists and the launch mob's id, read from `topology.live_snapshot`. */
export function listedMobsOf(experience: unknown): { mobs: MobDoorSource[] | null; primaryMobId: string | null } {
  const snapshot = record(record(record(experience)?.topology)?.live_snapshot);
  const mobs = Array.isArray(snapshot?.mobs) ? (snapshot.mobs as MobDoorSource[]) : null;
  return { mobs, primaryMobId: text(snapshot?.mob_id) || null };
}

/**
 * What a console connection knows of the gateway's mobs: whether the
 * gateway judges a request's mob, and the doors of the mobs it lists. The
 * transport seam keeps one per connection and reads every request pair and
 * every row key from it.
 */
export interface ConsoleMobAddressing {
  /** Whether the gateway lists {@link CONSOLE_MOB_ADDRESSING_CAPABILITY}; false until an answer says so. */
  readonly enabled: boolean;
  /**
   * Whether a `meta/capabilities` read was answered at all. Until then the
   * connection does not know whether the gateway hosts mobs behind doors
   * of their own: `enabled` is false because nothing said otherwise.
   */
  readonly known: boolean;
  /**
   * Whether a `meta/capabilities` read was answered: `enabled` is then the
   * gateway's own word. A console that went on without an answer
   * ({@link ConsoleMobAddressing.noteUnanswered}) is `known` and not
   * `answered`: nothing has said what the gateway hosts.
   */
  readonly answered: boolean;
  /**
   * The console stops waiting for a capability read: it goes on as with a
   * gateway that names no such capability, until a read answers. Takes
   * nothing back from an answer.
   */
  noteUnanswered(): void;
  readonly table: MobDoorTable;
  /** Take the feature names of a `meta/capabilities` answer. True when `enabled` changed. */
  noteFeatures(features: readonly string[] | null | undefined): boolean;
  /**
   * Take the mobs an experience lists, with the launch mob's id. `source`
   * is the experience document they were read from. A gateway leaves an
   * empty list out of its answer: where the capability is known, no list
   * is an empty list.
   */
  noteMobs(mobs: ReadonlyArray<MobDoorSource> | null | undefined, primaryMobId: string | null | undefined, source?: unknown): void;
  /**
   * The experience document the mobs were last taken from, or null when
   * none was taken since `enabled` last changed. A console holds a roster
   * of every mob the caller may address when the document it shows is this
   * one.
   */
  readonly listedSource: unknown;
  /**
   * Take what an earlier page load saw: mobs and the door each was served
   * through. The connection remembers by itself the mobs its own lists put
   * behind a hosted mob's door; this adds what the browser kept, so a mob
   * nested under a hosted mob that is asleep when the page loads still has
   * its door.
   */
  rememberBehind(seen: Iterable<readonly [string, string | null]>): void;
  /**
   * The mobs last taken, or null when none were taken since `enabled` last
   * changed: a list read before the change was asked the other way, and is
   * not a list of the mobs the caller may address.
   */
  readonly listedMobs: ReadonlyArray<MobDoorSource> | null;
  door(rowKey: string): MemberDoor;
  /**
   * Whether two rows of the roster spell this key for two members: no
   * reading of the key tells them apart, so a request for it is not sent.
   */
  sharedKey(rowKey: string): boolean;
  /**
   * Whether a name is known to be a member behind the launch mob's door:
   * the key of a listed row read on that door, or a key that names a listed
   * mob served through it.
   */
  behindTheLaunchDoor(name: string): boolean;
  rowKey(member: MemberOnTheWire): string | null;
  /**
   * A frame keyed by its member's row ({@link frameUnderRowKey}), or null
   * for a frame of a mob the list does not hold: the transport hands no
   * such frame on, and {@link onUnlistedMob} says that one was seen.
   */
  frame<F extends FrameOnTheWire>(frame: F, doorMobId?: string | null): F | null;
  /**
   * Called with the id of a mob a frame named and the list does not hold,
   * so the console reads its lists again: once for such a mob, and again
   * only when it was listed in between.
   */
  onUnlistedMob(listener: (mobId: string) => void): () => void;
  /** The selector a read over the host's values carries: `mobs=*` when enabled, else nothing. */
  everyMob(): { mobs?: typeof CONSOLE_MOBS_SELECTOR_EVERY };
  /** Whether an automatic request for this member is held back: its mob is listed as not loaded. */
  heldBack(rowKey: string): boolean;
  /** Called when `enabled` changes, so the console reads its lists again. */
  onChange(listener: () => void): () => void;
  /**
   * Called when the connection first knows how to go on (`known`: a
   * capability read answered, or the console went on without one), and once
   * more when a first answer comes after that (`answered`).
   */
  onKnown(listener: () => void): () => void;
}

export function createConsoleMobAddressing(): ConsoleMobAddressing {
  let enabled = false;
  let known = false;
  let answered = false;
  let listedSource: unknown = null;
  const knownListeners = new Set<() => void>();
  let mobs: ReadonlyArray<MobDoorSource> | null = null;
  let primary: string | null = null;
  let table: MobDoorTable = NO_MOB_DOORS;
  // The members the rows of the document the mobs were last taken from name.
  let rows: MemberRows = NO_MEMBER_ROWS;
  const listeners = new Set<() => void>();
  // The mobs seen behind a hosted mob's door, each with that door.
  const behind = new Map<string, string>();
  // The mobs frames named that the list did not hold, each said once.
  const unlistedSaid = new Set<string>();
  const unlistedListeners = new Set<(mobId: string) => void>();
  const rebuild = () => {
    table = mobDoorTable(mobs, primary, enabled, behind);
    rows = memberRowsOf(listedSource, table);
    for (const id of [...unlistedSaid]) {
      if (!enabled || table.doors.has(id)) unlistedSaid.delete(id);
    }
  };
  return {
    get enabled() {
      return enabled;
    },
    get table() {
      return table;
    },
    get listedMobs() {
      return mobs;
    },
    get known() {
      return known;
    },
    get answered() {
      return answered;
    },
    noteUnanswered() {
      if (known) return;
      known = true;
      for (const listener of [...knownListeners]) listener();
    },
    get listedSource() {
      return listedSource;
    },
    onKnown(listener) {
      knownListeners.add(listener);
      return () => {
        knownListeners.delete(listener);
      };
    },
    noteFeatures(features) {
      const next = Array.isArray(features) && features.includes(CONSOLE_MOB_ADDRESSING_CAPABILITY);
      // The first answer is told also to a console that went on without one.
      const first = !answered;
      known = true;
      answered = true;
      const changed = next !== enabled;
      if (changed) {
        enabled = next;
        // The list held was read the other way: it is not kept.
        mobs = null;
        listedSource = null;
        rebuild();
      }
      if (first) {
        for (const listener of [...knownListeners]) listener();
      }
      if (changed) {
        for (const listener of [...listeners]) listener();
      }
      return changed;
    },
    noteMobs(nextMobs, primaryMobId, source) {
      mobs = nextMobs ?? (enabled ? [] : null);
      listedSource = source ?? null;
      primary = text(primaryMobId) || null;
      if (enabled) {
        // What this list puts behind a hosted mob's door is remembered. A
        // mob remembered there and no longer listed stays only while that
        // hosted mob is listed and not loaded: once it is loaded, or gone,
        // the list says all there is.
        const listed = mobDoorTable(mobs, primary, true);
        for (const [id, door] of listed.doors) {
          if (door && door !== id) behind.set(id, door);
        }
        for (const [id, door] of [...behind]) {
          if (listed.doors.has(id)) continue;
          if (listed.doors.get(door) !== door || !listed.notLoaded.has(door)) behind.delete(id);
        }
      }
      rebuild();
    },
    rememberBehind(seen) {
      for (const [id, door] of seen) {
        if (id && door && door !== id && !behind.has(id)) behind.set(id, door);
      }
      rebuild();
    },
    door: (rowKey) => memberDoor(rowKey, table, rows),
    sharedKey: (rowKey) => rows.get(rowKey) === null,
    behindTheLaunchDoor(name) {
      const row = rows.get(name);
      if (row) return row.door === null;
      return row === undefined && memberMobId(name, table) !== null && memberDoor(name, table).mobId === null;
    },
    rowKey: (member) => memberRowKey(member, table),
    frame(frame, doorMobId) {
      const unlisted = frameUnlistedMob(frame, table, doorMobId);
      if (unlisted === null) return frameUnderRowKey(frame, table, doorMobId);
      if (!unlistedSaid.has(unlisted)) {
        unlistedSaid.add(unlisted);
        for (const listener of [...unlistedListeners]) listener(unlisted);
      }
      return null;
    },
    onUnlistedMob(listener) {
      unlistedListeners.add(listener);
      return () => {
        unlistedListeners.delete(listener);
      };
    },
    everyMob: () => (enabled ? { [CONSOLE_MOBS_SELECTOR_PARAM]: CONSOLE_MOBS_SELECTOR_EVERY } : {}),
    heldBack: (rowKey) => memberMobNotLoaded(rowKey, table, rows),
    onChange(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}
