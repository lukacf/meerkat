import type { ConsoleAccessSaveFailure } from "../types";

export function errorMessage(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

/// Extract the JSON-RPC error code from an error thrown by the console
/// transport (which annotates errors with `rpcError.code`). Returns null when
/// the error is not a typed JSON-RPC failure.
export function jsonRpcErrorCode(error: unknown): number | null {
  const rpcError = (error as { rpcError?: { code?: unknown } } | null)?.rpcError;
  return typeof rpcError?.code === "number" ? rpcError.code : null;
}

/// Extract the HTTP status the console transport annotates on non-OK RPC
/// responses (`httpStatus`). Returns null when the error did not come from an
/// HTTP-level rejection (network failure, timeout, or a JSON-RPC error body).
export function httpStatusCode(error: unknown): number | null {
  const status = (error as { httpStatus?: unknown } | null)?.httpStatus;
  return typeof status === "number" ? status : null;
}

/** Interpret only typed access-save feedback, never the error's message text.
 * An invalid configuration keeps the owner's reason token and detail: only an
 * administrator receives that refusal, and its detail names only entries of
 * the configuration that was saved (docs/concepts/access-control.mdx). */
export function accessSaveFailure(error: unknown): ConsoleAccessSaveFailure {
  const rpc = (error as { rpcError?: { code?: unknown; data?: { kind?: unknown; reason?: unknown; detail?: unknown } } } | null)?.rpcError;
  if (rpc?.code === -32009 && rpc.data?.kind === "access_revision_conflict") return { kind: "revision_conflict" };
  if (rpc?.code === -32009 && rpc.data?.kind === "access_owner_changed") return { kind: "owner_changed" };
  if (rpc?.code === -32004 && rpc.data?.kind === "access_mutation_unavailable") return { kind: "unavailable" };
  // The owner validated and refused the edit. An owner without checked saves
  // rejects the nested envelope as untyped invalid params instead.
  if (rpc?.code === -32602 && rpc.data?.kind === "invalid_access_config") {
    const { reason, detail } = rpc.data;
    return {
      kind: "invalid",
      ...(typeof reason === "string" && reason ? { reason } : {}),
      ...(typeof detail === "string" && detail.trim() ? { detail: detail.trim() } : {}),
    };
  }
  if (rpc?.code === -32602 || rpc?.code === -32601) return { kind: "unavailable" };
  return { kind: "failed" };
}

// The reasons an owner gives for refusing an invalid access configuration, in
// the Console's words. docs/concepts/access-control.mdx lists them, and the
// list can grow. `showDetail` marks the reasons whose detail names the rule,
// group or approver at fault, which the words cannot; the other details only
// restate the words.
const INVALID_ACCESS_REASONS = {
  enabled_without_admins: { words: "Enforcement needs at least one admin", showDetail: false },
  empty_rule_id: { words: "A rule needs an ID", showDetail: false },
  duplicate_rule_id: { words: "Two rules have the same ID", showDetail: true },
  empty_actions: { words: "A rule needs at least one permission", showDetail: true },
  unknown_action: { words: "A rule names a permission this runtime does not list", showDetail: true },
  unknown_group: { words: "A rule names a group that does not exist", showDetail: true },
  empty_approver_id: { words: "An approver needs an ID", showDetail: false },
  padded_approver_id: { words: "An approver ID starts or ends with a space", showDetail: true },
  approver_without_holders: { words: "An approver needs a person, service or group to hold it", showDetail: true },
  unknown_approver_group: { words: "An approver names a group that does not exist", showDetail: true },
  unknown_rule: { words: "The rule to delete does not exist", showDetail: true },
  invalid_toml: { words: "The access configuration file cannot hold this configuration", showDetail: false },
} as const satisfies Record<string, { words: string; showDetail: boolean }>;

function isKnownInvalidAccessReason(reason: string | undefined): reason is keyof typeof INVALID_ACCESS_REASONS {
  return reason !== undefined && Object.hasOwn(INVALID_ACCESS_REASONS, reason);
}

/// An invalid save says what is wrong: a known reason in the Console's words,
/// with the owner's detail where it names the entry at fault. For a reason
/// this Console does not know, the detail after the generic words says it;
/// without a detail, the generic words stand alone.
function invalidAccessNotice({ reason, detail }: ConsoleAccessSaveFailure): string {
  const known = isKnownInvalidAccessReason(reason) ? INVALID_ACCESS_REASONS[reason] : null;
  const words = known?.words ?? "The resulting access configuration is not valid";
  const shown = detail && (known === null || known.showDetail) ? ` (${detail})` : "";
  return `Changes were not saved. ${words}${shown}; your draft is retained for correction.`;
}

export function accessSaveNotice(failure: ConsoleAccessSaveFailure): string {
  switch (failure.kind) {
    case "revision_conflict": return "Access configuration changed. Review the latest settings before saving again.";
    case "owner_changed": return "The access configuration owner changed. Review the latest settings before saving again.";
    case "unavailable": return "Changes were not saved. Checked access saves are unavailable; your draft is retained.";
    case "invalid": return invalidAccessNotice(failure);
    case "failed": return "Changes were not saved. Your draft is retained; refresh Console access before trying again.";
  }
}
