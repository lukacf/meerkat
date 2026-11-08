import { CONSOLE_COMMAND_NAMES } from "./headless";

// ── WorkGraph operator action helpers ───────────────────────────────────────
//
// Console operator mutations are CAS-guarded: every request echoes the
// revision of the item (claim/close/release, goal confirm/request-close CAS
// against the goal WORK ITEM) or the binding's machine state (attention
// pause/resume/reassign) that the card or panel row showed. An action is
// never sent with a revision read at the click: that would apply it to a
// state the viewer never saw, so another person's change would go
// unnoticed. A card that showed no revision sends nothing; the console says
// so and reads the entity again, so the next action names a revision the
// viewer saw.

/// What the console says when a card or row showed no revision to act on.
export const WORKGRAPH_UNOBSERVED_REVISION_MESSAGE =
  "The console did not show the version this would change, so nothing was changed. It has been read again: check it, then try again.";

/// JSON-RPC code of the retryable CAS-conflict class every workgraph
/// mutation can return (`WORKGRAPH_CONFLICT_CODE` in rpc.rs, with
/// `data.kind = "workgraph_conflict"`).
export const WORKGRAPH_CONFLICT_CODE = -32042;

export interface WorkGraphConflictRefresh {
  command:
    | typeof CONSOLE_COMMAND_NAMES.workgraphGet
    | typeof CONSOLE_COMMAND_NAMES.workgraphGoalStatus;
  params: Record<string, unknown>;
}

/// A -32042 conflict proves the revision the inline card folded is stale —
/// and the fold only heals from observed frames, so without a fresh sighting
/// every subsequent click would resend the same wedged revision. Plan the
/// read that re-observes the entity the failed mutation addressed: item
/// mutations (`id`) via `meta/workgraph/get`; binding-addressed mutations
/// (`binding_id`) via `meta/workgraph/goal/status`, whose result carries
/// the goal item AND the binding machine state, healing both CAS classes at
/// once. Null when the failed params name no entity to refetch.
export function workGraphConflictRefreshRequest(
  params: Record<string, unknown>,
): WorkGraphConflictRefresh | null {
  const itemId = typeof params.id === "string" && params.id.trim() ? params.id : null;
  if (itemId) {
    return { command: CONSOLE_COMMAND_NAMES.workgraphGet, params: { id: itemId } };
  }
  const bindingId = typeof params.binding_id === "string" && params.binding_id.trim()
    ? params.binding_id
    : null;
  if (bindingId) {
    return { command: CONSOLE_COMMAND_NAMES.workgraphGoalStatus, params: { binding_id: bindingId } };
  }
  return null;
}

/// The part of a work item `workGraphClaimOwnedBy` reads.
export interface WorkGraphClaimedItem {
  claim?: { owner?: { key?: { kind?: string | null; id?: string | null } | null } | null } | null;
}

/// Whether `principal` owns the claim on `item`. The gateway owns a console
/// claim by its caller's principal; no principal owns nothing.
export function workGraphClaimOwnedBy(
  item: WorkGraphClaimedItem,
  principal: string | undefined,
): boolean {
  const key = item.claim?.owner?.key;
  return Boolean(principal) && key?.kind === "principal" && key.id === principal;
}

/// Whether the caller may release the claim on `item` from the console: the
/// claim's owner may, and so may an access admin (a caller with a principal
/// whom `whoami` reports as able to administer access), whose release the
/// gateway records with the claim it released. Nobody releases an item
/// without a claim.
export function workGraphMayRelease(
  item: WorkGraphClaimedItem,
  caller: { principal?: string; canAdminister?: boolean },
): boolean {
  if (!item.claim) return false;
  return workGraphClaimOwnedBy(item, caller.principal)
    || (Boolean(caller.principal) && caller.canAdminister === true);
}
