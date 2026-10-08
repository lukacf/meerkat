import type { WireAuthErrorReason } from "./generated/types.js";

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
