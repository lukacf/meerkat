// Removal of a shared record (meta/agent_memory/forget_in_scope) and the
// purge of a retired record's content (meta/agent_memory/purge): when each
// is offered and which scope the request names. Pure module (no React).

import type { MemoryFullRecord } from "../../types";

/// The scope a scoped memory request names, as the gateway parses it.
export type MemoryScopeParam =
  | { kind: "identity"; key: string }
  | { kind: "mob"; key: string }
  | { kind: "realm" };

type ScopedRecord = Pick<MemoryFullRecord, "scope">;
type RemovableRecord = Pick<MemoryFullRecord, "scope" | "status" | "title" | "body" | "purged">;

/// The request scope of a record. Null for a person's operator scope, which
/// neither call reaches.
export function memoryScopeParam(record: ScopedRecord): MemoryScopeParam | null {
  switch (record.scope.scope) {
    case "identity":
      return { kind: "identity", key: record.scope.identity };
    case "mob":
      return { kind: "mob", key: record.scope.mob };
    case "realm":
      return { kind: "realm" };
    default:
      return null;
  }
}

/// A purged record: retired, with no text left. The record read says so
/// (`purged`); a row without the flag is judged by its empty text.
export function isPurgedRecord(record: RemovableRecord): boolean {
  if (record.purged === true) return true;
  const retired = record.status.status === "tombstoned" || record.status.status === "superseded";
  return retired && record.title === "" && record.body === "";
}

/// Remove is offered for an active record of a shared scope: the mob's or
/// the realm's. An identity's own record is forgotten, not removed here.
export function canRemoveSharedRecord(record: RemovableRecord): boolean {
  return (
    record.status.status === "active" &&
    (record.scope.scope === "mob" || record.scope.scope === "realm")
  );
}

/// Purge is offered for a retired (tombstoned) record that still has text,
/// in a scope the call reaches.
export function canPurgeRecord(record: RemovableRecord): boolean {
  return (
    record.status.status === "tombstoned" &&
    memoryScopeParam(record) !== null &&
    !isPurgedRecord(record)
  );
}

/// What each action says before it is confirmed.
export const REMOVAL_NOTE = {
  remove:
    "Remove takes this record out of shared memory: agents are no longer given it. " +
    "Its text stays in the store until its content is purged.",
  purge:
    "Purge content blanks this record's title, body and tags, and those of the earlier " +
    "versions it replaced, for good. Copies made from it, proposals and conversation " +
    "transcripts are not touched.",
} as const;
