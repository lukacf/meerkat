import type { WireAuthErrorDetail, WireAuthErrorReason } from "./generated/types.js";

/** Every typed auth error reason, as the server's closed vocabulary. */
export const WIRE_AUTH_ERROR_REASONS: readonly WireAuthErrorReason[] = [
  "invalid_target",
  "realm_not_found",
  "binding_not_found",
  "binding_invalid",
  "binding_inherited",
  "flow_unsupported",
  "mcp_server_not_configured",
  "mcp_server_mismatch",
  "account_selection_required",
  "unknown_strategy",
  "attempt_missing",
  "attempt_mismatch",
  "device_poll_in_progress",
  "device_code_already_admitted",
  "device_expiry_invalid",
  "account_mismatch",
  "missing_scopes",
  "credential_mismatch",
  "verification_unavailable",
  "slot_occupied",
  "slot_account_mismatch",
  "slot_context_mismatch",
  "slot_mode_mismatch",
  "unverified_connector_publication",
  "reauth_required",
  "authorization_required",
  "callback_unavailable",
  "upstream_failure",
  "configuration_invalid",
  "credential_access_denied",
  "credential_interaction_required",
  "credential_access_pending",
  "credential_store_unavailable",
  "infrastructure",
];

/**
 * The typed reason of an auth error (`auth/*` RPC methods carry it in
 * `error.data.reason`, which the SDK keeps as `details`). Branch on it, never
 * on the error text. `undefined` for errors without a known reason.
 */
export function authErrorReason(error: unknown): WireAuthErrorReason | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const details = (error as { details?: unknown }).details;
  if (typeof details !== "object" || details === null) return undefined;
  const reason = (details as { reason?: unknown }).reason;
  return typeof reason === "string" &&
    (WIRE_AUTH_ERROR_REASONS as readonly string[]).includes(reason)
    ? (reason as WireAuthErrorReason)
    : undefined;
}

/** Every stable auth refusal sub-kind (`error.data.detail`). */
export const WIRE_AUTH_ERROR_DETAILS: readonly WireAuthErrorDetail[] = [
  "remote_custody_required",
];

/**
 * The stable sub-kind of an auth refusal, beside its reason (`auth/*` RPC
 * methods carry it in `error.data.detail`). For example
 * `remote_custody_required` (reason `invalid_target`): an MCP login started
 * on the host's registered https callback was completed without its
 * `host_attempt_handle`, for example after a restart; cancel the attempt by
 * its reference and start again. `undefined` when absent.
 */
export function authErrorDetail(error: unknown): WireAuthErrorDetail | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const details = (error as { details?: unknown }).details;
  if (typeof details !== "object" || details === null) return undefined;
  const detail = (details as { detail?: unknown }).detail;
  return typeof detail === "string" &&
    (WIRE_AUTH_ERROR_DETAILS as readonly string[]).includes(detail)
    ? (detail as WireAuthErrorDetail)
    : undefined;
}
