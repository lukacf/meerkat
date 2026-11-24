// What the console remembers of the mobs it listed, so it can tell a mob
// created again under the id of a destroyed one from the mob it knew.
//
// A listed mob carries an incarnation token: opaque, and different for a
// mob created again under an id. The console keeps, per user scope, the
// token and the door of each mob it last listed. When a listed mob's token
// differs from the one kept, or a mob the host created is no longer in the
// roster of every mob, the mob is forgotten: what the browser holds for its
// members (queued messages, drafts, open chats) is dropped before anything
// of a new mob under that id is shown. Nothing is sent for it.
//
// A mob that is only asleep stays listed, and nothing of it is forgotten.
// A mob a member created is listed only while it is held and one of its
// members is seen, and not at all while the hosted mob it is nested under
// sleeps: a roster without it does not say it is gone, and it is kept as it
// was seen. A gateway that sends no tokens (it does not host mobs behind
// doors of their own) is never asked to forget.
//
// Pure but for the two storage functions, which take the storage.

import type { MobDoorSource, MobDoorTable } from "../../../packages/console-core/src/mob-door";

export const MOB_INCARNATIONS_STORAGE_PREFIX = "meta-console-mob-incarnations";

/** One mob as the console last listed it. */
export interface SeenMob {
  incarnation: string;
  /** The mob whose door served it: itself, another listed mob, or null for the launch mob's door. */
  door: string | null;
}

/** The mobs the console last listed, by id. */
export type SeenMobs = Readonly<Record<string, SeenMob>>;

interface MobIncarnationStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
}

/** The key the seen mobs of one user scope are kept under. */
export function mobIncarnationsStorageKey(namespace: string): string {
  return `${MOB_INCARNATIONS_STORAGE_PREFIX}:${encodeURIComponent(namespace)}`;
}

/** The seen mobs kept under `key`; none when nothing readable is kept. */
export function readSeenMobs(storage: MobIncarnationStorage | null | undefined, key: string): SeenMobs {
  if (!storage) return {};
  try {
    const parsed: unknown = JSON.parse(storage.getItem(key) || "null");
    const mobs = parsed && typeof parsed === "object" ? (parsed as { mobs?: unknown }).mobs : null;
    if (!mobs || typeof mobs !== "object") return {};
    const seen: Record<string, SeenMob> = {};
    for (const [id, value] of Object.entries(mobs as Record<string, unknown>)) {
      const entry = value && typeof value === "object" ? value as { incarnation?: unknown; door?: unknown } : null;
      if (!id || !entry || typeof entry.incarnation !== "string" || !entry.incarnation) continue;
      seen[id] = { incarnation: entry.incarnation, door: typeof entry.door === "string" && entry.door ? entry.door : null };
    }
    return seen;
  } catch {
    return {};
  }
}

/** Keep the seen mobs under `key`. */
export function writeSeenMobs(storage: MobIncarnationStorage | null | undefined, key: string, seen: SeenMobs): void {
  if (!storage) return;
  try {
    storage.setItem(key, JSON.stringify({ v: 1, mobs: seen }));
  } catch {
    /* Storage denied: the next list is compared with nothing, and nothing is forgotten. */
  }
}

/** What a list of mobs means for the mobs the console had seen. */
export interface SeenMobsUpdate {
  /** The mobs to forget: created again under their id, or no longer listed. */
  forget: string[];
  /** The mobs seen now. */
  seen: SeenMobs;
  /** Whether `seen` differs from what was seen before. */
  changed: boolean;
}

/** How many mobs that are not listed the record keeps: the most recently first seen. */
const UNLISTED_SEEN_LIMIT = 200;

/**
 * Compares the listed mobs (`listed`, with their doors in `table`) with the
 * mobs seen before, and says which to forget.
 *
 * The signal that a mob is another mob is its token: a listed mob is
 * forgotten when its token differs from the one seen.
 *
 * Absence from a roster means "gone" only for a mob the host created: the
 * roster of every mob is the list of the hosted mobs the caller may
 * address, asleep ones included, so a hosted mob it does not hold is gone
 * for this caller, and so is what sat behind its door. A mob a member
 * created is listed only while it is held and one of its members is listed
 * to the caller: a roster can come without it though it still exists. Such
 * a mob is kept as it was seen (behind the launch mob's door, or behind a
 * hosted mob that is listed and the same), and forgotten only when it comes
 * back with another token, or when the hosted mob it sat behind is gone or
 * was created again.
 *
 * A mob first seen is recorded and nothing of it is forgotten; a listed mob
 * with no token keeps the record it had. The launch mob is never forgotten:
 * its members' keys name no mob. The record keeps a bounded number of mobs
 * that are not listed.
 */
export function updateSeenMobs(seenBefore: SeenMobs, listed: ReadonlyArray<MobDoorSource>, table: MobDoorTable): SeenMobsUpdate {
  const forget: string[] = [];
  const listedIds = new Set<string>();
  const listedNow = new Map<string, SeenMob>();
  for (const mob of listed) {
    const id = typeof mob.mob_id === "string" ? mob.mob_id.trim() : "";
    if (!id || listedIds.has(id)) continue;
    listedIds.add(id);
    const incarnation = typeof mob.incarnation === "string" ? mob.incarnation : "";
    if (!incarnation || id === table.launchMobId) continue;
    const before = seenBefore[id];
    if (before && before.incarnation !== incarnation) forget.push(id);
    listedNow.set(id, { incarnation, door: table.doors.get(id) ?? null });
  }
  const recreated = new Set(forget);
  // In the order first seen: the mobs seen before, then the new ones.
  const seen: Record<string, SeenMob> = {};
  const unlisted: string[] = [];
  for (const [id, before] of Object.entries(seenBefore)) {
    const now = listedNow.get(id);
    if (now) {
      seen[id] = now;
      continue;
    }
    if (listedIds.has(id)) {
      // Listed with no token: nothing says it is another mob.
      if (id !== table.launchMobId) seen[id] = before;
      continue;
    }
    const hosted = before.door === id;
    const doorGone = before.door !== null && before.door !== id && (!listedIds.has(before.door) || recreated.has(before.door));
    if (hosted || doorGone) {
      forget.push(id);
      continue;
    }
    seen[id] = before;
    unlisted.push(id);
  }
  for (const [id, now] of listedNow) {
    if (!(id in seen)) seen[id] = now;
  }
  for (const id of unlisted.slice(0, Math.max(0, unlisted.length - UNLISTED_SEEN_LIMIT))) delete seen[id];
  const changed = forget.length > 0
    || Object.keys(seen).length !== Object.keys(seenBefore).length
    || Object.entries(seen).some(([id, mob]) => seenBefore[id]?.incarnation !== mob.incarnation || seenBefore[id]?.door !== mob.door);
  return { forget, seen, changed };
}

/**
 * The key prefixes of the members of the mob `mobId` whose door is `door`
 * (contract v0.6.0, `mob_addressing.row_key`): `<mob id>/` for a hosted
 * mob (every member behind its door, those of the mobs nested under it
 * too) and for a mob nested under the launch mob; `<door>/<mob id>/` for a
 * mob nested under a hosted mob, and `<mob id>/` beside it for the keys an
 * earlier console gave such members.
 */
export function memberKeyPrefixes(mobId: string, door: string | null | undefined): string[] {
  return door && door !== mobId ? [`${door}/${mobId}/`, `${mobId}/`] : [`${mobId}/`];
}

/** Whether a member's key is of the mob `mobId` (behind the door `door`, when it has one): see {@link memberKeyPrefixes}. */
export function memberOfMob(identity: string | null | undefined, mobId: string, door?: string | null): boolean {
  const key = (identity || "").trim();
  return memberKeyPrefixes(mobId, door).some((prefix) => key.startsWith(prefix));
}
