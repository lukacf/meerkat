/**
 * Human-readable summary of a failure frame's payload.
 *
 * meerkat >= 0.7 `run_failed` (projected by Meta as `interaction_failed`)
 * carries its failure truth as the typed `error_report { class, reason,
 * message }`; the gateway also derives flat `error` and `reason` strings
 * from it. Frames persisted before that derivation, and frames Meta mints
 * itself (`reason: "superseded_by_later_run"`, `error: "<delivery failure>"`),
 * carry only one of the shapes, so every failure renderer reads them all in
 * one place: the typed report first, then the flat keys.
 */
export interface FailureSummary {
  /** Operator-readable message, or "" when the payload names none. */
  message: string;
  /** Stable discriminator (`reason.reason_type`, a flat `reason`, or a typed reason `kind`), or "". */
  reasonType: string;
}

function recordOf(value: unknown): Record<string, unknown> | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function nonEmptyString(value: unknown): string {
  return typeof value === "string" && value.trim() ? value : "";
}

export function summarizeFailureData(data: unknown): FailureSummary {
  const record = recordOf(data);
  if (!record) return { message: "", reasonType: "" };
  const report = recordOf(record.error_report);
  const reason = recordOf(report?.reason) ?? recordOf(record.reason);
  const reasonType =
    nonEmptyString(reason?.reason_type)
    || nonEmptyString(record.reason)
    || nonEmptyString(reason?.kind);
  const message =
    nonEmptyString(report?.message)
    || nonEmptyString(record.error)
    || nonEmptyString(record.message);
  return { message, reasonType };
}

/**
 * `message (reasonType)` when both are known and distinct, else whichever is
 * known, else `fallback`. Never renders an object.
 */
export function describeFailure(data: unknown, fallback = "error"): string {
  const { message, reasonType } = summarizeFailureData(data);
  if (message && reasonType && !message.includes(reasonType)) return `${message} (${reasonType})`;
  return message || reasonType || fallback;
}
