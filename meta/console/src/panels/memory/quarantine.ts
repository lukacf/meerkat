// The operator half of quarantine review (meta/memory/quarantine/decide):
// when a decision is offered, how a rationale is limited, and what a
// decision or a refusal says. Pure module (no React).

import { errorMessage } from "../../lib/errors";
import { formatAbsoluteTime, humanizeEnum } from "../../../../packages/console-components/src/panel-kit/format";
import type { MemoryFullRecord, MemoryQuarantineDecideResult, MemoryRecordStatus } from "../../types";
import { STATUS_WORD } from "./words";

/// A Biography offers the quarantine decision for a quarantined
/// identity-scope record whose detail carries the content hash to bind.
export function canDecideQuarantinedRecord(record: MemoryFullRecord): boolean {
  return (
    record.status.status === "quarantined" &&
    record.scope.scope === "identity" &&
    typeof record.content_hash === "string" &&
    record.content_hash.length > 0
  );
}

/// The server's MAX_REVIEW_RATIONALE_BYTES: a review rationale is limited in
/// UTF-8 bytes of its trimmed text, not in characters.
export const QUARANTINE_RATIONALE_MAX_BYTES = 400;

/// Why a rationale cannot be sent as typed, or null when it fits. Measured
/// on the trimmed text, which is what the request carries.
export function quarantineRationaleProblem(rationale: string): string | null {
  const bytes = new TextEncoder().encode(rationale.trim()).length;
  return bytes > QUARANTINE_RATIONALE_MAX_BYTES
    ? `The rationale is ${bytes} bytes (UTF-8); the limit is ${QUARANTINE_RATIONALE_MAX_BYTES}.`
    : null;
}

/// A message about a quarantine decision: a plain sentence, plus the record
/// or promotion IDs it concerns, which render as copyable technical IDs
/// after the sentence instead of inside it. `title` is the sentence's
/// tooltip: the grant names behind an access refusal.
export interface QuarantineNotice {
  text: string;
  ids: Array<{ label: string; value: string }>;
  title?: string;
}

/// A record status said in a sentence ("discarded", "replaced"), for the
/// statuses a refusal reports.
function statusInSentence(status: unknown): string {
  const known = STATUS_WORD[String(status) as MemoryRecordStatus["status"]];
  return known ? known.toLowerCase() : humanizeEnum(String(status)).toLowerCase();
}

/// What a quarantine decision did. The panel then opens the record it left
/// active (the release) or the decided record itself.
export function quarantineDecisionSummary(result: MemoryQuarantineDecideResult): QuarantineNotice {
  const successor = result.successor?.memory_id;
  const successorIds = successor ? [{ label: "New record", value: successor }] : [];
  switch (result.outcome) {
    case "released":
      return {
        text: "Released. A new active record holds this memory at observed trust, marked as once quarantined; the quarantined original is kept, marked as discarded.",
        ids: successorIds,
      };
    case "already_released":
      return { text: "Already released earlier; nothing changed.", ids: successorIds };
    case "tombstoned": {
      const invalidated = result.decision?.review?.invalidated_promotions ?? [];
      return {
        text:
          invalidated.length === 0
            ? "Discarded; it will never be recalled."
            : `Discarded; it will never be recalled. ${
                invalidated.length === 1
                  ? "The move to team memory waiting on it was cancelled"
                  : `The ${invalidated.length} moves to team memory waiting on it were cancelled`
              } and can no longer publish.`,
        ids: invalidated.map((id) => ({ label: "Cancelled move", value: id })),
      };
    }
    case "already_tombstoned":
      return { text: "Already discarded; nothing changed.", ids: [] };
    default:
      return { text: `Decided: ${humanizeEnum(String(result.outcome)).toLowerCase()}.`, ids: [] };
  }
}

/// The grants a quarantine decision needs, for the refusal's tooltip.
const QUARANTINE_DECISION_GRANTS =
  "Grants: memory.quarantine.review, agent.view and agent.memory.read, plus agent.memory.write to release or agent.memory.delete to discard (tombstone)";

/// A failed quarantine decision in words: the typed refusal reasons of
/// meta/memory/quarantine/decide, the access denial, or the raw error.
export function quarantineDecisionErrorText(error: unknown): QuarantineNotice {
  const rpcError = (error as { rpcError?: { code?: unknown; data?: unknown } } | null)?.rpcError;
  if (rpcError?.code === -32030) {
    return {
      text: "No access. To decide, you need permission to review quarantined memory and to view this agent and read its memory. Releasing also needs permission to write its memory, and discarding to delete its memory.",
      ids: [],
      title: QUARANTINE_DECISION_GRANTS,
    };
  }
  const data = (rpcError?.data ?? null) as Record<string, unknown> | null;
  if (data?.kind === "memory_quarantine_review_refused") {
    switch (data.reason) {
      case "not_found":
        return { text: "Refused: this record is no longer in the agent's memory.", ids: [] };
      case "content_mismatch":
        return {
          text: "Refused: the record differs from the one loaded here. Reload it and review again.",
          ids: [],
        };
      case "not_quarantined":
        return data.released_as
          ? {
              text: "Refused: it was already released.",
              ids: [{ label: "Released as", value: String(data.released_as) }],
            }
          : { text: `Refused: the record is ${statusInSentence(data.status)}, not quarantined.`, ids: [] };
      case "gate_pending": {
        const expires =
          typeof data.expires_at_ms === "number" && data.expires_at_ms > 0
            ? `, or wait until it expires on ${formatAbsoluteTime(data.expires_at_ms)}`
            : "";
        return {
          text: `Refused: this record is waiting for approval to move to team memory. Decide it in Approvals${expires}. Discarding the record instead cancels the move.`,
          ids: data.pending_id ? [{ label: "Waiting move", value: String(data.pending_id) }] : [],
        };
      }
      case "successor_conflict":
        return {
          text: "Refused: another record already holds the ID this release would create; nothing was changed.",
          ids: data.successor_id ? [{ label: "Conflicting record", value: String(data.successor_id) }] : [],
        };
      case "secret_detected":
        return {
          text: `Refused: the content matches the ${String(data.class)} secret pattern; it can only be discarded.`,
          ids: [],
        };
      case "stale_update":
        return {
          text: `Refused: the version this record replaces is ${statusInSentence(data.prior_status)}; discard it instead.`,
          ids: data.prior ? [{ label: "Replaced version", value: String(data.prior) }] : [],
        };
      default:
        break;
    }
  }
  return { text: errorMessage(error), ids: [] };
}
