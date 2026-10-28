import type { CommsSendErrorData } from "./generated/types.js";

// Checked against the generated union at compile time: a missing or extra
// code fails the build.
const COMMS_SEND_ERROR_CODE_TABLE = {
  peer_not_found_or_not_trusted: true,
  peer_unreachable: true,
  peer_admission_dropped: true,
  peer_input_rejected: true,
  peer_wake_refused: true,
  send_ambiguous: true,
  send_failed: true,
  invalid_command: true,
} as const satisfies Record<CommsSendErrorData["code"], true>;

/** Every typed `comms/send` error code, as the server's closed vocabulary. */
export const COMMS_SEND_ERROR_CODES = Object.freeze(
  Object.keys(COMMS_SEND_ERROR_CODE_TABLE),
) as readonly CommsSendErrorData["code"][];

/** Whether `code` is a typed `comms/send` error code. */
export function isCommsSendErrorCode(code: unknown): code is CommsSendErrorData["code"] {
  return (
    typeof code === "string" &&
    Object.prototype.hasOwnProperty.call(COMMS_SEND_ERROR_CODE_TABLE, code)
  );
}

/**
 * The typed data of a failed `comms/send` (`error.data`, tagged on `code`,
 * which the SDK keeps as `details`). Branch on `code`, never on the error
 * text. `undefined` for errors without a known `comms/send` code.
 */
export function commsSendError(error: unknown): CommsSendErrorData | undefined {
  if (typeof error !== "object" || error === null) return undefined;
  const details = (error as { details?: unknown }).details;
  if (typeof details !== "object" || details === null || Array.isArray(details)) return undefined;
  return isCommsSendErrorCode((details as { code?: unknown }).code)
    ? (details as CommsSendErrorData)
    : undefined;
}
