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

/** Interpret only typed access-save feedback, never private server message text. */
export function accessSaveFailure(error: unknown): ConsoleAccessSaveFailure {
  const rpc = (error as { rpcError?: { code?: unknown; data?: { kind?: unknown } } } | null)?.rpcError;
  if (rpc?.code === -32009 && rpc.data?.kind === "access_revision_conflict") return { kind: "revision_conflict" };
  if (rpc?.code === -32009 && rpc.data?.kind === "access_owner_changed") return { kind: "owner_changed" };
  if (rpc?.code === -32004 && rpc.data?.kind === "access_mutation_unavailable") return { kind: "unavailable" };
  // The owner validated and refused the edit. An owner without checked saves
  // rejects the nested envelope as untyped invalid params instead.
  if (rpc?.code === -32602 && rpc.data?.kind === "invalid_access_config") return { kind: "invalid" };
  if (rpc?.code === -32602 || rpc?.code === -32601) return { kind: "unavailable" };
  return { kind: "failed" };
}

export function accessSaveNotice(failure: ConsoleAccessSaveFailure): string {
  switch (failure.kind) {
    case "revision_conflict": return "Access configuration changed. Review the latest settings before saving again.";
    case "owner_changed": return "The access configuration owner changed. Review the latest settings before saving again.";
    case "unavailable": return "Changes were not saved. Checked access saves are unavailable; your draft is retained.";
    case "invalid": return "Changes were not saved. The resulting access configuration is not valid; your draft is retained for correction.";
    case "failed": return "Changes were not saved. Your draft is retained; refresh Console access before trying again.";
  }
}
