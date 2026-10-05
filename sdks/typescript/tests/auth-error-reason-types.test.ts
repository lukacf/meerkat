import { authErrorReason, MeerkatError, WIRE_AUTH_ERROR_REASONS } from "../src/index.js";
import type { WireAuthErrorReason } from "../src/index.js";

type Equal<A, B> =
  (<T>() => T extends A ? 1 : 2) extends
  (<T>() => T extends B ? 1 : 2) ? true : false;
const listIsComplete: Equal<(typeof WIRE_AUTH_ERROR_REASONS)[number], WireAuthErrorReason> = true;

/** A host's exhaustive handling: a new reason fails to type-check here. */
function retryWithKnownAccount(reason: WireAuthErrorReason): boolean {
  switch (reason) {
    case "slot_occupied":
    case "slot_account_mismatch":
      return true;
    case "invalid_target":
    case "realm_not_found":
    case "binding_not_found":
    case "binding_invalid":
    case "binding_inherited":
    case "flow_unsupported":
    case "mcp_server_not_configured":
    case "mcp_server_mismatch":
    case "account_selection_required":
    case "unknown_strategy":
    case "attempt_missing":
    case "attempt_mismatch":
    case "device_poll_in_progress":
    case "device_code_already_admitted":
    case "device_expiry_invalid":
    case "account_mismatch":
    case "missing_scopes":
    case "credential_mismatch":
    case "verification_unavailable":
    case "slot_context_mismatch":
    case "slot_mode_mismatch":
    case "unverified_connector_publication":
    case "reauth_required":
    case "authorization_required":
    case "callback_unavailable":
    case "upstream_failure":
    case "configuration_invalid":
    case "infrastructure":
      return false;
    default: {
      const unreachable: never = reason;
      return unreachable;
    }
  }
}

const error = new MeerkatError("-32602", "slot is occupied", { reason: "slot_occupied" });
const reason: WireAuthErrorReason | undefined = authErrorReason(error);
// @ts-expect-error An unknown string is not a reason.
const invalid: WireAuthErrorReason = "slot_full";
void [listIsComplete, retryWithKnownAccount, reason, invalid];
