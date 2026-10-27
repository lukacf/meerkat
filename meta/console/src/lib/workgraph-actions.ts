import { CONSOLE_COMMAND_NAMES } from "./headless";

// ── WorkGraph operator action helpers ───────────────────────────────────────
//
// Console operator mutations are CAS-guarded: every request must echo the
// latest revision of the item (claim/close, goal confirm/request-close CAS
// against the goal WORK ITEM) or the binding's machine state (attention
// pause/resume/reassign). When the UI never observed a revision, these
// helpers fetch the live one instead of guessing — sending 0 is a guaranteed
// conflict. Resolution failures throw so callers surface the error banner
// and never dispatch the mutation.

/// Console-issued workgraph command executor, already bound to the workgraph
/// workbench target. Injected so the resolution paths are testable.
export type WorkGraphCommandRunner = (
  command:
    | typeof CONSOLE_COMMAND_NAMES.workgraphGet
    | typeof CONSOLE_COMMAND_NAMES.workgraphGoalStatus,
  params: Record<string, unknown>,
) => Promise<unknown>;

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

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;
}

function revisionOfItem(result: unknown): number | undefined {
  const item = asRecord(asRecord(result)?.item);
  return typeof item?.revision === "number" ? item.revision : undefined;
}

/// Live revision of one work item via `meta/workgraph/get`.
export async function resolveWorkGraphItemRevision(
  run: WorkGraphCommandRunner,
  itemId: string,
): Promise<number> {
  const result = await run(CONSOLE_COMMAND_NAMES.workgraphGet, { id: itemId });
  const revision = revisionOfItem(result);
  if (revision === undefined) {
    throw new Error(`could not resolve the current revision of work item ${itemId}`);
  }
  return revision;
}

/// Live revision of the goal WORK ITEM behind an attention binding via
/// `meta/workgraph/goal/status` — the CAS token goal confirm/request-close
/// must echo.
export async function resolveWorkGraphGoalItemRevision(
  run: WorkGraphCommandRunner,
  bindingId: string,
): Promise<number> {
  const result = await run(CONSOLE_COMMAND_NAMES.workgraphGoalStatus, { binding_id: bindingId });
  const revision = revisionOfItem(result);
  if (revision === undefined) {
    throw new Error(`could not resolve the goal item revision for binding ${bindingId}`);
  }
  return revision;
}

/// Live machine revision of an attention binding via
/// `meta/workgraph/goal/status` — the CAS token attention
/// pause/resume/reassign must echo.
export async function resolveWorkGraphBindingRevision(
  run: WorkGraphCommandRunner,
  bindingId: string,
): Promise<number> {
  const result = await run(CONSOLE_COMMAND_NAMES.workgraphGoalStatus, { binding_id: bindingId });
  const machineState = asRecord(asRecord(asRecord(result)?.attention)?.machine_state);
  const revision = typeof machineState?.revision === "number" ? machineState.revision : undefined;
  if (revision === undefined) {
    throw new Error(`could not resolve the machine revision of attention binding ${bindingId}`);
  }
  return revision;
}

/// Owner id stamped on console-issued claims. The authenticated operator
/// (the experience `access.subject`, same identity the console ABAC layer
/// authorizes against) wins; the static ops-lead id is only a fallback for
/// runtimes without console auth.
export function workGraphClaimOwnerId(
  subject: string | null | undefined,
  fallback: string,
): string {
  const trimmed = typeof subject === "string" ? subject.trim() : "";
  return trimmed || fallback;
}
