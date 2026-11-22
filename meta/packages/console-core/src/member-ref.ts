// The console's one owner of the identity of a member of a nested mob.
//
// A console names the members of its own mob (the primary mob) by their bare
// names. A member of any other mob the gateway lists is named by its mob and
// its name in one string, `<mob id>/<member name>`, in every identity field
// the console reads or writes: rows, topology nodes, frames, sends, wired
// peers and the creator a mob names. The console treats an identity as an
// opaque key and sends it back as it came; it reads the form only to show a
// name for people.
//
// The form is unambiguous because a mob id never holds `/`: the first `/` of
// a nested member's identity ends its mob id, and the name after it may hold
// any character, a `/` included. Parsing is syntactic. Whether the part
// before the first `/` names a mob the console lists is the caller's
// question: a runtime namespace prefixes an identity in the same way
// (`east/reviewer`), so {@link memberDisplayName} drops the mob only for a
// mob the console lists.
//
// This mirrors the gateway's pair in
// meta/crates/meerkat-meta/src/console_member_ref.rs over the shared cases in
// console_member_ref_cases.json. No other console code composes or splits the
// form. Pure, with no runtime imports, so every test bundle can include it by
// a relative path.

/** The separator between a nested mob's id and its member's name. */
const SEPARATOR = "/";

/**
 * The console identity of the member `name` of the nested mob `mobId`:
 * `<mob id>/<name>`. Null when either part is empty or the mob id holds the
 * separator: such a member has no console identity.
 */
export function nestedMemberIdentity(mobId: string, name: string): string | null {
  if (!mobId || !name || mobId.includes(SEPARATOR)) return null;
  return `${mobId}${SEPARATOR}${name}`;
}

/**
 * The mob id and the member name a nested member's console identity holds,
 * split at its first `/`. Null for a string with no `/`, or with nothing
 * before or after the first one: a bare name, which names a primary member.
 */
export function parseNestedMemberIdentity(identity: string): { mobId: string; name: string } | null {
  const at = identity.indexOf(SEPARATOR);
  if (at <= 0 || at === identity.length - 1) return null;
  return { mobId: identity.slice(0, at), name: identity.slice(at + 1) };
}

/**
 * The member name to show for the console identity `identity` of a member of
 * the mob `mobId`: the name without the mob for a nested member's identity of
 * that mob, else the identity as it is.
 */
export function memberNameInMob(identity: string, mobId: string | null | undefined): string {
  if (!mobId) return identity;
  const parsed = parseNestedMemberIdentity(identity);
  return parsed && parsed.mobId === mobId ? parsed.name : identity;
}

/**
 * The ids of the mobs a console lists other than its own (`primaryMobId`):
 * the mobs whose members carry the `<mob id>/<member name>` identity.
 */
export function nestedMobIds(
  mobs: ReadonlyArray<{ mob_id?: string | null }> | null | undefined,
  primaryMobId: string | null | undefined,
): Set<string> {
  const primary = primaryMobId?.trim() || "";
  const ids = new Set<string>();
  for (const mob of mobs || []) {
    const id = typeof mob?.mob_id === "string" ? mob.mob_id.trim() : "";
    if (id && id !== primary) ids.add(id);
  }
  return ids;
}

/**
 * The same set read from the roster alone: a mob is a listed nested mob when
 * a listed member of it carries its id in its identity. A console lists a
 * mob only through a member the caller sees, so both readings agree; this
 * one serves a surface that holds the roster and not the mob list.
 */
export function nestedMobIdsOfMembers(
  members: ReadonlyArray<{ identity?: string | null; member_id?: string | null; mob_id?: string | null }> | null | undefined,
): Set<string> {
  const ids = new Set<string>();
  for (const member of members || []) {
    const mobId = typeof member?.mob_id === "string" ? member.mob_id.trim() : "";
    if (!mobId || ids.has(mobId)) continue;
    const identity = (member.identity || member.member_id || "").trim();
    if (parseNestedMemberIdentity(identity)?.mobId === mobId) ids.add(mobId);
  }
  return ids;
}

/**
 * The member name to show for a console identity when no roster row names
 * the member: the name without the mob when the identity is a member's of a
 * listed nested mob (`nested`, see {@link nestedMobIds}), else the identity
 * as it is. The identity itself stays the key, the tooltip and what a copy
 * action copies.
 */
export function memberDisplayName(identity: string, nested: ReadonlySet<string> | null | undefined): string {
  if (!nested || nested.size === 0) return identity;
  const parsed = parseNestedMemberIdentity(identity);
  return parsed && nested.has(parsed.mobId) ? parsed.name : identity;
}

/** The last path segment of a peer reference: "team/role/member" reads "member". */
export function referenceLastSegment(reference: string): string {
  const at = reference.lastIndexOf(SEPARATOR);
  return at >= 0 && at < reference.length - 1 ? reference.slice(at + 1) : reference;
}

/** The keys a reference to a member is looked up by, in order. */
export interface MemberReferenceKeys {
  /** Identities to look up exactly, the likeliest first. */
  keys: string[];
  /**
   * The reference starts with a listed nested mob: it names a member of that
   * mob or no one, so nothing looser than `keys` may match it.
   */
  nested: boolean;
}

/**
 * How a reference to a member (a console identity, or a peer name such as
 * `<mob id>/<role>/<member>`) is looked up among the members a console
 * lists. A reference that starts with a listed nested mob (`nested`) names
 * that mob's member only: its keys are the reference and
 * `<mob id>/<last segment>`, and no looser match applies, so
 * `alpha/reviewer` never names the `reviewer` of another mob. Any other
 * reference keeps the lookups it always had: itself, then its last path
 * segment (a member of the console's own mob).
 */
export function memberReferenceKeys(
  reference: string,
  nested: ReadonlySet<string> | null | undefined,
): MemberReferenceKeys {
  const trimmed = reference.trim();
  const last = referenceLastSegment(trimmed);
  const parsed = nested && nested.size > 0 ? parseNestedMemberIdentity(trimmed) : null;
  if (parsed && nested!.has(parsed.mobId)) {
    const short = nestedMemberIdentity(parsed.mobId, last);
    return { keys: short && short !== trimmed ? [trimmed, short] : [trimmed], nested: true };
  }
  return { keys: last && last !== trimmed ? [trimmed, last] : [trimmed], nested: false };
}
