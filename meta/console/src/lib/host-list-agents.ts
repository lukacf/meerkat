import type { ConsoleAttentionSnapshot } from "../../../packages/console-core/src/console-attention";
import type { PendingApproval } from "../../../packages/console-core/src/pending-approvals";
import type { QuestionSet, QuestionSetSnapshot } from "../../../packages/console-core/src/pending-questions";

// The host's lists name an agent in their entries: the questions agents
// asked, the approvals, the person's replies. The contract says an entry
// names its agent by the pair of a frame: the mob beside the name (absent
// for the launch mob's door). The console places an entry both ways,
// fail-safe:
//
// - an entry that CARRIES a mob names one agent: it is handed on under that
//   agent's key (placed in that agent's chat, shown and opened as it); a
//   mob the console does not list names no agent;
// - an entry that carries NO mob is resolved to an agent only where its
//   name can be one agent (mob-door: hostListNameAmbiguous); else it is
//   handed on with words in the agent's place that are no agent's key, so
//   it is placed in no chat, shown under no agent's name, and opens none.
//   An entry of another mob that arrives without its mob is so never read
//   as the launch mob's agent where the name is shared or a mob is not
//   loaded.
//
// `place` does that for one name and the mob its entry carries. Each
// function returns its input itself when nothing changed, so a list whose
// names stay as they are costs no re-render.

/** A name of an entry, and the mob the entry carries beside it (if any), as the console hands it on. */
export type PlaceHostListAgent = (name: string, mobId?: string) => string;

const RAW_AGENT_WORDS = ["actor_id", "agent", "identity", "actor"] as const;

/**
 * An entry as the gateway wrote it (a pending approval, a record of the
 * gating audit), with each word that names its agent as the console hands
 * it on. The mob the entry carries is `origin.mob_id`. The entry itself
 * when no name changed.
 */
function placedRawEntry(entry: Readonly<Record<string, unknown>>, place: PlaceHostListAgent): Readonly<Record<string, unknown>> {
  const rawOrigin = entry.origin && typeof entry.origin === "object" && !Array.isArray(entry.origin)
    ? entry.origin as Record<string, unknown>
    : null;
  const carried = rawOrigin?.mob_id;
  const mobId = typeof carried === "string" && carried.trim() ? carried.trim() : undefined;
  let placed: Record<string, unknown> | null = null;
  for (const word of RAW_AGENT_WORDS) {
    const named = entry[word];
    if (typeof named !== "string" || !named.trim()) continue;
    const handedOn = place(named.trim(), mobId);
    if (handedOn === named) continue;
    placed ??= { ...entry };
    placed[word] = handedOn;
  }
  const named = rawOrigin?.identity;
  if (rawOrigin && typeof named === "string" && named.trim()) {
    const handedOn = place(named.trim(), mobId);
    if (handedOn !== named) {
      placed ??= { ...entry };
      placed.origin = { ...rawOrigin, identity: handedOn };
    }
  }
  return placed ?? entry;
}

function placedApproval(request: PendingApproval, place: PlaceHostListAgent): PendingApproval {
  const mobId = request.origin?.mobId;
  const actor = request.actorId === undefined ? undefined : place(request.actorId, mobId);
  const origin = request.origin === undefined ? undefined : place(request.origin.identity, mobId);
  if (actor === request.actorId && origin === request.origin?.identity) return request;
  return {
    ...request,
    ...(actor !== undefined ? { actorId: actor } : {}),
    ...(request.origin && origin !== undefined ? { origin: { ...request.origin, identity: origin } } : {}),
    // The entry as the gateway wrote it is shown too (the Approvals view): the same names there.
    raw: placedRawEntry(request.raw, place),
  };
}

/** The records of the gating audit with each record's agent as the console hands it on. */
export function auditWithPlacedAgents(entries: readonly unknown[], place: PlaceHostListAgent): readonly unknown[] {
  let changed = false;
  const placed = entries.map((entry) => {
    if (!entry || typeof entry !== "object" || Array.isArray(entry)) return entry;
    const handedOn = placedRawEntry(entry as Record<string, unknown>, place);
    if (handedOn !== entry) changed = true;
    return handedOn;
  });
  return changed ? placed : entries;
}

/** The approvals with each entry's agent as the console hands it on. */
export function approvalsWithPlacedAgents<S extends { requests: readonly PendingApproval[] }>(snapshot: S, place: PlaceHostListAgent): S {
  let changed = false;
  const requests = snapshot.requests.map((request) => {
    const placed = placedApproval(request, place);
    if (placed !== request) changed = true;
    return placed;
  });
  return changed ? { ...snapshot, requests } : snapshot;
}

function placedQuestionSet(set: QuestionSet, place: PlaceHostListAgent): QuestionSet {
  const identity = place(set.origin.identity, set.origin.mobId);
  return identity === set.origin.identity ? set : { ...set, origin: { ...set.origin, identity } };
}

/** The question sets with each asker as the console hands it on. */
export function questionSetsWithPlacedAgents<V extends { snapshot: QuestionSetSnapshot }>(view: V, place: PlaceHostListAgent): V {
  let changed = false;
  const sets = view.snapshot.sets.map((set) => {
    const placed = placedQuestionSet(set, place);
    if (placed !== set) changed = true;
    return placed;
  });
  const closed = view.snapshot.closed.map((entry) => {
    const placed = placedQuestionSet(entry.set, place);
    if (placed === entry.set) return entry;
    changed = true;
    return { ...entry, set: placed };
  });
  return changed ? { ...view, snapshot: { ...view.snapshot, sets, closed } } : view;
}

/** The person's replies with each agent as the console hands it on. */
export function attentionWithPlacedAgents(snapshot: ConsoleAttentionSnapshot, place: PlaceHostListAgent): ConsoleAttentionSnapshot {
  let changed = false;
  const items = snapshot.items.map((item) => {
    const identity = place(item.origin.identity, item.origin.mobId);
    if (identity === item.origin.identity) return item;
    changed = true;
    return { ...item, origin: { ...item.origin, identity } };
  });
  return changed ? { ...snapshot, items } : snapshot;
}
